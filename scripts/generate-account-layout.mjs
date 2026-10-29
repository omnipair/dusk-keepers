/**
 * Emit byte offsets for the account fields keepers read.
 *
 * A keeper needs a handful of fields out of accounts with dozens: a market's
 * vault addresses, a position's owner and collateral. Decoding the whole
 * account would mean a second Rust model of every protocol struct, free to
 * drift from the program with each release.
 *
 * Reading by offset is the alternative, and hand-counted offsets are worse
 * than either — `Market.quote_side.asset_mint` sits behind a nested struct
 * whose size nobody can eyeball, and a wrong offset is a silent wrong answer
 * rather than an error. So the offsets are computed here from the pinned IDL,
 * which already describes every field in declaration order, and regenerating
 * after a redeploy is what keeps them true.
 *
 *   node scripts/generate-account-layout.mjs [--write]
 */

import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";

const root = new URL("../", import.meta.url);
const outputPath = new URL("protocol/keeper-account-layout.v1.json", root);

/** Accounts and the fields keepers actually read. */
const WANTED = {
  BorrowPosition: [
    "owner",
    "market",
    "position_id",
    "base_collateral",
    "quote_collateral",
    "base_referral_partner",
    "quote_referral_partner",
    "fixed_base_shares",
    "fixed_quote_shares",
    "active_liquidation_session",
    "base_distress.active",
    "base_distress.started_at",
    "quote_distress.active",
    "quote_distress.started_at",
  ],
  Market: [
    "ylp_mint",
    "base_side.hlp_mint",
    "quote_side.hlp_mint",
    "base_side.asset_mint",
    "base_side.asset_decimals",
    "base_side.reserve_vault",
    "base_side.collateral_vault",
    "base_side.interest_vault",
    "quote_side.asset_mint",
    "quote_side.asset_decimals",
    "quote_side.reserve_vault",
    "quote_side.collateral_vault",
    "quote_side.interest_vault",
    "base_side.reserves.live_reserve",
    "quote_side.reserves.live_reserve",
    "base_hlp_vault.ylp_vault",
    "quote_hlp_vault.ylp_vault",
    "base_hlp_vault.hlp_supply",
    "base_hlp_vault.debt_principal",
    "quote_hlp_vault.debt_principal",
    "base_hlp_vault.debt_shares",
    "quote_hlp_vault.debt_shares",
    "quote_hlp_vault.hlp_supply",
    "base_hlp_vault.residual_exposure",
    "quote_hlp_vault.residual_exposure",
    "insurance.base_vault",
    "insurance.quote_vault",
    "params_hash",
    "last_update_slot",
  ],
  LeveragePosition: [
    "owner",
    "market",
    "position_id",
    "debt_asset",
    "namespace_authority",
    "referral_partner",
    "collateral_amount",
    "debt_shares",
    "active_liquidation_session",
    "distress.active",
    "distress.started_at",
  ],
  // Only the fields ahead of `update`, which is a data-carrying enum and so
  // gives everything after it no fixed offset. The lifecycle keeper does not
  // need the rest: whether a proposal may be queued or executed is the
  // program's judgement, asked by simulation, not a status byte read here.
  FutarchyAuthority: [
    "recipients.futarchy_treasury",
    "fee_auction.accepted_mint",
    "fee_auction.recipients.treasury",
    "fee_auction.recipients.staking_vault",
    "buyback_auction.accepted_mint",
    "buyback_auction.recipients.treasury",
    "buyback_auction.recipients.staking_vault",
  ],
  ParameterProposal: ["market", "proposer", "nonce"],
};

const ORDER_WANTED = {
  LeverageOrder: [
    "owner",
    "market",
    "position",
    "order_id",
    "kind",
    "close_bps",
  ],
  LeverageEntryOrder: [
    "owner",
    "market",
    "position",
    "position_id",
    "debt_mint",
    "collateral_mint",
    "order_id",
    "debt_asset",
    "margin_amount",
    "executor_bounty",
    "expiry_unix_timestamp",
  ],
  HlpOrder: [
    "owner",
    "market",
    "target_hlp_mint",
    "custody_hlp_account",
    "order_id",
    "kind",
    "status",
    "hlp_amount",
  ],
};

/** Fixed sizes for the primitives the IDL uses. */
const PRIMITIVE_SIZE = {
  bool: 1,
  i8: 1,
  i16: 2,
  i32: 4,
  i64: 8,
  i128: 16,
  pubkey: 32,
  u8: 1,
  u16: 2,
  u32: 4,
  u64: 8,
  u128: 16,
};

function sizeOf(type, types, path) {
  if (typeof type === "string") {
    const size = PRIMITIVE_SIZE[type];
    assert.ok(size !== undefined, `${path}: unsupported primitive ${type}`);
    return size;
  }
  if (type.array) {
    const [inner, count] = type.array;
    assert.equal(
      typeof count,
      "number",
      `${path}: only fixed-length arrays are sized`,
    );
    return sizeOf(inner, types, path) * count;
  }
  if (type.defined) {
    const name = type.defined.name ?? type.defined;
    const defined = types.get(name);
    assert.ok(defined, `${path}: ${name} is absent from the IDL`);
    // An enum's discriminant is one byte, and every keeper-read enum here is
    // fieldless. A data-carrying one would not have a fixed size at all, so it
    // is rejected rather than guessed at.
    if (defined.type.kind === "enum") {
      for (const variant of defined.type.variants) {
        assert.ok(
          !variant.fields,
          `${path}: ${name} carries data and has no fixed size`,
        );
      }
      return 1;
    }
    return structSize(defined, types, `${path}.${name}`);
  }
  if (type.option) {
    throw new Error(`${path}: an option has no fixed offset for later fields`);
  }
  throw new Error(`${path}: unsupported type ${JSON.stringify(type)}`);
}

function structSize(defined, types, path) {
  return defined.type.fields.reduce(
    (total, field) =>
      total + sizeOf(field.type, types, `${path}.${field.name}`),
    0,
  );
}

/** Walk a dotted path, accumulating offsets, and return the leaf's location. */
function locate(accountName, dotted, types) {
  // Anchor prefixes every account with an eight-byte discriminator.
  let offset = 8;
  let current = types.get(accountName);
  assert.ok(current, `${accountName} is absent from the IDL`);

  const segments = dotted.split(".");
  for (const [index, segment] of segments.entries()) {
    let found;
    for (const field of current.type.fields) {
      if (field.name === segment) {
        found = field;
        break;
      }
      offset += sizeOf(field.type, types, `${accountName}.${field.name}`);
    }
    assert.ok(found, `${accountName}.${dotted}: no field named ${segment}`);
    if (index === segments.length - 1) {
      return {
        offset,
        size: sizeOf(found.type, types, `${accountName}.${dotted}`),
      };
    }
    const name = found.type.defined?.name ?? found.type.defined;
    current = types.get(name);
    assert.ok(
      current,
      `${accountName}.${dotted}: ${name} is absent from the IDL`,
    );
  }
  throw new Error(`${accountName}.${dotted}: unreachable`);
}

async function build(programName = "dusk", wanted = WANTED) {
  const lock = JSON.parse(
    await readFile(new URL("protocol.lock.json", root), "utf8"),
  );
  const dusk = lock.programs.find((program) => program.name === programName);
  assert.ok(dusk, "protocol lock does not pin the dusk program");
  const idl = JSON.parse(await readFile(new URL(dusk.idl.path, root), "utf8"));
  const types = new Map(idl.types.map((type) => [type.name, type]));

  const accounts = Object.entries(wanted).map(([accountName, fields]) => {
    const defined = types.get(accountName);
    assert.ok(defined, `${accountName} is absent from the pinned IDL`);
    const located = fields.map((field) => ({
      path: field,
      ...locate(accountName, field, types),
    }));

    // The full size doubles as a decode guard: a short account is a different
    // account, not a truncated one. Some accounts carry a data-bearing enum
    // and have no single size, so those record the minimum the requested
    // fields imply instead. The weaker guard is stated rather than a strong
    // one being faked.
    let exactSize = null;
    try {
      exactSize = 8 + structSize(defined, types, accountName);
    } catch {
      exactSize = null;
    }

    return {
      name: accountName,
      ...(exactSize === null
        ? {
            minimumSize: located.reduce(
              (highest, field) => Math.max(highest, field.offset + field.size),
              0,
            ),
          }
        : { sizeWithDiscriminator: exactSize }),
      fields: located,
    };
  });

  return {
    $schema: "./schemas/account-layout.schema.json",
    schemaVersion: 1,
    protocolRevision: lock.revision,
    programId: dusk.programId,
    idlSha256: dusk.idl.sha256,
    accounts,
  };
}

for (const [programName, wanted, path] of [
  ["dusk", WANTED, outputPath],
  [
    "leverage_delegate",
    ORDER_WANTED,
    new URL("protocol/keeper-order-layout.v1.json", root),
  ],
]) {
  const layout = await build(programName, wanted);
  const formatted = `${JSON.stringify(layout, null, 2)}\n`;

  if (process.argv.includes("--write")) {
    await writeFile(path, formatted);
    console.log(
      `wrote offsets for ${layout.accounts.length} accounts (${layout.accounts.reduce((total, account) => total + account.fields.length, 0)} fields)`,
    );
  } else {
    assert.equal(
      await readFile(path, "utf8"),
      formatted,
      "account layout is stale",
    );
    console.log(`verified offsets for ${layout.accounts.length} accounts`);
  }
}
