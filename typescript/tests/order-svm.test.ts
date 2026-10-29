import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:http";
import bs58 from "bs58";
import { createHash } from "node:crypto";
import anchor from "@coral-xyz/anchor";
import { IdlCoder } from "@coral-xyz/anchor/dist/cjs/coder/borsh/idl.js";
import {
  PublicKey,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
  AddressLookupTableAccount,
} from "@solana/web3.js";
import {
  AccountLayout,
  TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountIdempotentInstruction,
} from "@solana/spl-token";
import { bank, result } from "./helpers/liquidation-bank.ts";

const dusk = JSON.parse(readFileSync("artifacts/dusk.json", "utf8"));
const delegate = JSON.parse(
  readFileSync("artifacts/leverage_delegate.json", "utf8"),
);
const program = new PublicKey(dusk.address);
const delegatedProgram = new PublicKey(delegate.address);
const bn = (value: number | bigint) => new anchor.BN(value.toString());
const key = (name: string) =>
  new PublicKey(createHash("sha256").update(name).digest());
const pda = (p: PublicKey, ...seeds: (string | Buffer | PublicKey)[]) =>
  PublicKey.findProgramAddressSync(
    seeds.map((s) =>
      typeof s === "string"
        ? Buffer.from(s)
        : s instanceof PublicKey
          ? s.toBuffer()
          : s,
    ),
    p,
  );
const u64 = (v: number) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(v));
  return b;
};
function codec(idl: any, name: string) {
  return IdlCoder.typeDefLayout({
    typeDef: idl.types.find((t: any) => t.name === name),
    types: idl.types,
  });
}
function decode(idl: any, name: string, raw: Uint8Array) {
  return codec(idl, name).decode(Buffer.from(raw).subarray(8));
}
function data(idl: any, name: string, fields: any) {
  const layout = codec(idl, name);
  const value = Object.assign(layout.decode(Buffer.alloc(32768)), fields);
  const raw = Buffer.alloc(32768);
  const size = layout.encode(value, raw);
  return Buffer.concat([
    Buffer.from(idl.accounts.find((a: any) => a.name === name).discriminator),
    raw.subarray(0, size),
  ]);
}
function put(svm: any, address: PublicKey, owner: PublicKey, raw: Buffer) {
  svm.setAccount(address, {
    owner,
    data: raw,
    lamports: 10000000n,
    executable: false,
  });
}
function token(svm: any, owner: PublicKey, mint: PublicKey, amount: bigint) {
  const address = getAssociatedTokenAddressSync(mint, owner, true);
  const raw = Buffer.alloc(AccountLayout.span);
  AccountLayout.encode(
    {
      mint,
      owner,
      amount,
      delegateOption: 0,
      delegate: PublicKey.default,
      state: 1,
      isNativeOption: 0,
      isNative: 0n,
      delegatedAmount: 0n,
      closeAuthorityOption: 0,
      closeAuthority: PublicKey.default,
    },
    raw,
  );
  put(svm, address, TOKEN_PROGRAM_ID, raw);
  return address;
}
function snapshot(svm: any, keys: string[]) {
  return {
    slot: Number(svm.getClock().slot),
    accounts: Object.fromEntries(
      [...new Set(keys)].map((k) => {
        const a = svm.getAccount(new PublicKey(k));
        return [
          k,
          !a || a.lamports === 0n
            ? null
            : {
                owner: a.owner.toBase58(),
                data: Array.from(a.data),
                executable: a.executable,
              },
        ];
      }),
    ),
  };
}
function plan(
  svm: any,
  fixture: any,
  order: PublicKey,
  kind: string,
  executor: PublicKey,
) {
  const input = {
    order: order.toBase58(),
    kind,
    executor: executor.toBase58(),
    snapshot: snapshot(svm, [...Object.keys(fixture.bank), order.toBase58()]),
  };
  const call = () =>
    JSON.parse(
      execFileSync("target/debug/dusk-keeper", ["--order-plan"], {
        input: JSON.stringify(input),
        encoding: "utf8",
      }),
    );
  let value = call();
  input.snapshot = snapshot(svm, [
    ...Object.keys(input.snapshot.accounts),
    ...Object.values(value.named).map((k: any) => new PublicKey(k).toBase58()),
  ]);
  value = call();
  const instructions = value.instructions.map(
    (ix: any) =>
      new TransactionInstruction({
        programId: new PublicKey(ix.program_id),
        data: Buffer.from(ix.data),
        keys: ix.accounts.map((a: any) => ({
          pubkey: new PublicKey(a.pubkey),
          isSigner: a.is_signer,
          isWritable: a.is_writable,
        })),
      }),
  );
  const addresses: PublicKey[] = [
    ...new Map(
      instructions
        .flatMap((i: any) => i.keys)
        .filter((a: any) => !a.isSigner)
        .map((a: any) => [a.pubkey.toBase58(), a.pubkey]),
    ).values(),
  ] as PublicKey[];
  const table = key("order lookup");
  const raw = Buffer.alloc(56 + 32 * addresses.length);
  raw.writeUInt32LE(1);
  raw.writeBigUInt64LE(0xffffffffffffffffn, 4);
  addresses.forEach((k, i) => k.toBuffer().copy(raw, 56 + 32 * i));
  put(
    svm,
    table,
    new PublicKey("AddressLookupTab1e1111111111111111111111111"),
    raw,
  );
  const tx = new VersionedTransaction(
    new TransactionMessage({
      payerKey: executor,
      recentBlockhash: svm.latestBlockhash(),
      instructions,
    }).compileToV0Message([
      new AddressLookupTableAccount({
        key: table,
        state: {
          deactivationSlot: 0xffffffffffffffffn,
          lastExtendedSlot: 0,
          lastExtendedSlotStartIndex: 0,
          authority: undefined,
          addresses,
        },
      }),
    ]),
  );
  assert.ok(tx.serialize().length <= 1232);
  return { tx, value };
}

function entrySetup(
  options: { limit?: bigint; expiry?: number; multiplier?: number } = {},
) {
  const f = JSON.parse(
    readFileSync("fixtures/litesvm/07-flash-LeveragePosition.json", "utf8"),
  );
  const svm = bank(f);
  const original = decode(dusk, "LeveragePosition", f.bank[f.position].data);
  const market = original.market as PublicKey;
  const m = decode(dusk, "Market", svm.getAccount(market)!.data);
  const owner = key("order owner");
  svm.setAccount(owner, {
    owner: PublicKey.default,
    data: Buffer.alloc(0),
    lamports: 1000000000n,
    executable: false,
  });
  f.bank[owner.toBase58()] = {};
  const executor = new PublicKey(f.buyer);
  const [order, bump] = pda(
    delegatedProgram,
    "leverage_entry_order",
    market,
    owner,
    u64(99),
  );
  const positionId = key("keeper order position");
  const [position] = pda(
    program,
    "leverage_position_v2",
    market,
    owner,
    order,
    positionId,
  );
  const margin = 1000;
  token(svm, order, m.quote_side.asset_mint, 1200n);
  const raw = data(delegate, "LeverageEntryOrder", {
    owner,
    market,
    position,
    position_id: positionId,
    debt_mint: m.quote_side.asset_mint,
    collateral_mint: m.base_side.asset_mint,
    order_id: bn(99),
    debt_asset: 1,
    margin_amount: bn(margin),
    executor_bounty: bn(10),
    multiplier_bps: bn(options.multiplier ?? 20000),
    limit_price_nad: bn(options.limit ?? 0xffffffffffffffffn),
    min_collateral_out: bn(1),
    expiry_unix_timestamp: bn(options.expiry ?? 1000000000000),
    referrer: null,
    bump,
  });
  put(svm, order, delegatedProgram, raw);
  const prepared = plan(svm, f, order, "LeverageEntryOrder", executor);
  const rewardBefore = Buffer.from(
    svm.getAccount(new PublicKey(prepared.value.reward_account))?.data ??
      Buffer.alloc(165),
  ).readBigUInt64LE(64);
  return {
    f,
    svm,
    market,
    owner,
    executor,
    order,
    position,
    prepared,
    rewardBefore,
  };
}

test("native entry executor opens position, pays bounty and refunds escrow", () => {
  const c = entrySetup();
  assert.equal(result(c.svm, c.prepared.tx), null);
  assert.equal(c.svm.getAccount(c.order)?.lamports ?? 0n, 0n);
  const position = decode(
    dusk,
    "LeveragePosition",
    c.svm.getAccount(c.position)!.data,
  );
  assert.equal(position.owner.toBase58(), c.owner.toBase58());
  assert.equal(position.namespace_authority.toBase58(), c.order.toBase58());
  assert.equal(position.margin_amount.toString(), "1000");
  assert.equal(
    Buffer.from(
      c.svm.getAccount(new PublicKey(c.prepared.value.reward_account))!.data,
    ).readBigUInt64LE(64) - c.rewardBefore,
    10n,
  );
});

for (const [name, options, expected] of [
  ["unmet limit", { limit: 1n }, "SlippageExceeded"],
  ["expired", { expiry: 1 }, "InvalidOrder"],
] as const)
  test(`native entry ${name} preserves escrow`, () => {
    const c = entrySetup(options);
    const before = Buffer.from(c.svm.getAccount(c.order)!.data);
    const failure = result(c.svm, c.prepared.tx);
    assert.ok(failure, `${name} must reject`);
    assert.ok(
      failure.logs.some((l: string) => l.includes(expected)),
      JSON.stringify(failure),
    );
    assert.deepEqual(Buffer.from(c.svm.getAccount(c.order)!.data), before);
    assert.equal(c.svm.getAccount(c.position)?.lamports ?? 0n, 0n);
  });

for (const kind of [1, 2])
  for (const closeBps of [5000, 10000])
    test(`native ${kind === 1 ? "TP" : "SL"} closes ${closeBps} bps and settles bounty`, () => {
      const c = entrySetup();
      const entryResult = result(c.svm, c.prepared.tx);
      assert.equal(entryResult, null, JSON.stringify(entryResult));
      const pos = decode(
        dusk,
        "LeveragePosition",
        c.svm.getAccount(c.position)!.data,
      );
      const [order, bump] = pda(
        delegatedProgram,
        "leverage_order",
        c.position,
        c.owner,
        u64(100),
      );
      const [delegation, delegationBump] = pda(
        program,
        "leverage_delegation_v2",
        c.position,
      );
      put(
        c.svm,
        delegation,
        program,
        data(dusk, "LeverageDelegation", {
          owner: c.owner,
          market: c.market,
          position: c.position,
          debt_asset: 1,
          delegated_program: delegatedProgram,
          approved_actions: 1,
          open_curve_revision: pos.open_curve_revision,
          bump: delegationBump,
        }),
      );
      put(
        c.svm,
        order,
        delegatedProgram,
        data(delegate, "LeverageOrder", {
          owner: c.owner,
          market: c.market,
          position: c.position,
          open_curve_revision: pos.open_curve_revision,
          order_id: bn(100),
          kind,
          trigger_closeout_price_nad: bn(kind === 1 ? 1 : 0xffffffffffffffffn),
          close_bps: closeBps,
          bump,
        }),
      );
      c.f.bank[delegation.toBase58()] = {};
      c.f.bank[c.position.toBase58()] = {};
      const { tx, value } = plan(
        c.svm,
        c.f,
        order,
        "LeverageOrder",
        c.executor,
      );
      const rewardKey = new PublicKey(value.reward_account);
      const before = Buffer.from(
        c.svm.getAccount(rewardKey)!.data,
      ).readBigUInt64LE(64);
      const failure = result(c.svm, tx);
      assert.equal(failure, null, JSON.stringify(failure));
      assert.equal(c.svm.getAccount(order)?.lamports ?? 0n, 0n);
      assert.ok(
        Buffer.from(c.svm.getAccount(rewardKey)!.data).readBigUInt64LE(64) >
          before,
      );
      if (closeBps === 10000)
        assert.equal(c.svm.getAccount(c.position)?.lamports ?? 0n, 0n);
      else
        assert.ok(
          decode(
            dusk,
            "LeveragePosition",
            c.svm.getAccount(c.position)!.data,
          ).collateral_amount.lt(pos.collateral_amount),
        );
    });

function hlpSetup(side: number, kind = 1, trigger = 0xffffffffffffffffn) {
  const f = JSON.parse(
    readFileSync("fixtures/hlp-litesvm/01-hlp_flash.json", "utf8"),
  );
  const svm = bank(f);
  const market = new PublicKey(f.market);
  const m = decode(dusk, "Market", svm.getAccount(market)!.data);
  const target = side === 0 ? m.base_side.hlp_mint : m.quote_side.hlp_mint;
  const owner = key("hLP order owner");
  const executor = new PublicKey(f.buyer);
  const [order, bump] = pda(
    delegatedProgram,
    "hlp_order",
    market,
    owner,
    target,
    u64(101),
  );
  const receiptMint = svm.getAccount(target)!;
  const custody = getAssociatedTokenAddressSync(
    target,
    order,
    true,
    receiptMint.owner,
  );
  const amount = 1000000n;
  // This liquidation capture omits holder receipt accounts. Represent a
  // small part of the existing mint supply in valid native-created custody.
  const setup = new VersionedTransaction(
    new TransactionMessage({
      payerKey: executor,
      recentBlockhash: svm.latestBlockhash(),
      instructions: [
        createAssociatedTokenAccountIdempotentInstruction(
          executor,
          custody,
          order,
          target,
          receiptMint.owner,
        ),
      ],
    }).compileToV0Message(),
  );
  const setupFailure = result(svm, setup);
  assert.equal(setupFailure, null, JSON.stringify(setupFailure));
  const escrow = Buffer.from(svm.getAccount(custody)!.data);
  escrow.writeBigUInt64LE(amount, 64);
  put(svm, custody, receiptMint.owner, escrow);
  f.bank[custody.toBase58()] = {};
  for (const mint of [m.base_side.asset_mint, m.quote_side.asset_mint]) {
    const [yieldKey, yieldBump] = pda(
      program,
      "yield",
      market,
      order,
      target,
      mint,
      Buffer.from([1]),
    );
    put(
      svm,
      yieldKey,
      program,
      data(dusk, "YieldAccount", {
        owner: order,
        market,
        lp_mint: target,
        asset_mint: mint,
        token_kind: 1,
        recipient: owner,
        bump: yieldBump,
        harvest_authority: null,
      }),
    );
    f.bank[yieldKey.toBase58()] = {};
  }
  svm.setAccount(owner, {
    owner: PublicKey.default,
    data: Buffer.alloc(0),
    lamports: 1000000000n,
    executable: false,
  });
  f.bank[owner.toBase58()] = {};
  put(
    svm,
    order,
    delegatedProgram,
    data(delegate, "HlpOrder", {
      owner,
      market,
      target_hlp_mint: target,
      custody_hlp_account: custody,
      order_id: bn(101),
      kind,
      status: 0,
      hlp_amount: bn(amount),
      trigger_nad: bn(trigger),
      min_target_amount_out: bn(1),
      bump,
    }),
  );
  const prepared = plan(svm, f, order, "HlpOrder", executor);
  return { f, svm, order, prepared, target };
}

for (const side of [0, 1])
  for (const kind of [1, 2])
    test(`native hLP side ${side} stop ${kind} redeems escrow and pays bounty`, () => {
      const c = hlpSetup(side, kind, kind === 1 ? 0xffffffffffffffffn : 0n);
      const reward = new PublicKey(c.prepared.value.reward_account);
      const before = Buffer.from(
        c.svm.getAccount(reward)?.data ?? Buffer.alloc(165),
      ).readBigUInt64LE(64);
      const failure = result(c.svm, c.prepared.tx);
      assert.equal(failure, null, JSON.stringify(failure));
      assert.ok(
        Buffer.from(c.svm.getAccount(reward)!.data).readBigUInt64LE(64) >
          before,
      );
      if ((c.svm.getAccount(c.order)?.lamports ?? 0n) > 0n) {
        const o = decode(delegate, "HlpOrder", c.svm.getAccount(c.order)!.data);
        assert.equal(o.status, 2);
        assert.equal(
          o.hlp_amount.toString(),
          "1000000",
          "receipt amount remains historical; executed status prevents reuse",
        );
      }
      const custody = new PublicKey(c.prepared.value.named.custody_hlp_account);
      assert.equal(
        Buffer.from(
          c.svm.getAccount(custody)?.data ?? Buffer.alloc(165),
        ).readBigUInt64LE(64),
        0n,
      );
    });

test("native hLP unmet stop preserves escrow", () => {
  const c = hlpSetup(0, 1, 1n);
  const before = Buffer.from(c.svm.getAccount(c.order)!.data);
  const failure = result(c.svm, c.prepared.tx);
  assert.ok(
    failure?.logs.some((line: string) => line.includes("TriggerNotMet")),
    JSON.stringify(failure),
  );
  assert.deepEqual(Buffer.from(c.svm.getAccount(c.order)!.data), before);
});

for (const kind of ["LeverageEntryOrder", "HlpOrder"])
  test(`native leverage worker discovers and simulates ${kind} through RPC`, async () => {
    const c = kind === "HlpOrder" ? hlpSetup(0) : entrySetup();
    const svm = c.svm;
    const keys = [...Object.keys(c.f.bank), c.order.toBase58()];
    let simulations = 0;
    const rpcAccount = (a: any) => {
      if (!a) return null;
      const field = (n: string) => (typeof a[n] === "function" ? a[n]() : a[n]);
      return field("lamports") > 0
        ? {
            lamports: Number(field("lamports")),
            owner: new PublicKey(field("owner")).toBase58(),
            executable: field("executable"),
            data: [Buffer.from(field("data")).toString("base64"), "base64"],
          }
        : null;
    };
    const server = createServer(async (req, res) => {
      let call: any;
      try {
        const chunks = [];
        for await (const chunk of req) chunks.push(chunk);
        call = JSON.parse(Buffer.concat(chunks).toString());
        const p = call.params;
        const context = { slot: Number(svm.getClock().slot) };
        let answer: any;
        if (call.method === "getSlot") answer = context.slot;
        else if (call.method === "getMultipleAccounts")
          answer = {
            context,
            value: p[0].map((k: string) =>
              rpcAccount(svm.getAccount(new PublicKey(k))),
            ),
          };
        else if (call.method === "getProgramAccounts") {
          const allowed = [
            "Market",
            "LeverageOrder",
            "LeverageEntryOrder",
            "HlpOrder",
          ].map((n) =>
            createHash("sha256").update(`account:${n}`).digest().subarray(0, 8),
          );
          answer = keys
            .map((pubkey) => ({
              pubkey,
              account: rpcAccount(svm.getAccount(new PublicKey(pubkey))),
            }))
            .filter(
              (a) =>
                a.account?.owner === p[0] &&
                allowed.some((d) =>
                  d.equals(
                    Buffer.from(a.account.data[0], "base64").subarray(0, 8),
                  ),
                ) &&
                bs58.encode(
                  Buffer.from(a.account.data[0], "base64").subarray(0, 8),
                ) === p[1].filters[0].memcmp.bytes,
            );
          if (p[1].dataSlice)
            answer = answer.map((a: any) => ({
              ...a,
              account: { ...a.account, data: ["", "base64"] },
            }));
        } else if (call.method === "simulateTransaction") {
          simulations++;
          const outcome: any = svm.simulateTransaction(
            VersionedTransaction.deserialize(Buffer.from(p[0], "base64")),
          );
          const failed = typeof outcome.err === "function";
          const posts = new Map<string, any>(
            failed
              ? []
              : outcome
                  .postAccounts()
                  .map(([k, a]: any) => [new PublicKey(k).toBase58(), a]),
          );
          answer = {
            context,
            value: {
              err: failed ? String(outcome.err()) : null,
              logs: outcome.meta().logs(),
              accounts: (p[1].accounts?.addresses ?? []).map((k: string) =>
                rpcAccount(
                  posts.has(k)
                    ? posts.get(k)
                    : svm.getAccount(new PublicKey(k)),
                ),
              ),
            },
          };
        } else throw new Error(`unexpected RPC ${call.method}`);
        res.setHeader("content-type", "application/json");
        res.end(
          JSON.stringify({ jsonrpc: "2.0", id: call.id, result: answer }),
        );
      } catch (error) {
        res.end(
          JSON.stringify({
            jsonrpc: "2.0",
            id: call?.id ?? 1,
            error: { message: String(error) },
          }),
        );
      }
    });
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    try {
      const run = await new Promise<{ code: number | null; output: string }>(
        (resolve, reject) => {
          const proc = spawn("target/debug/dusk-keeper", ["--once"], {
            env: {
              ...process.env,
              KEEPER_MODE: "shadow",
              KEEPER_PROFILE: "leverage",
              KEEPER_SHADOW_PUBLIC_KEY: c.f.buyer,
              PORT: "0",
              SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${(server.address() as any).port}`,
              KEEPER_LIQUIDATION_LOOKUP_TABLE: key("order lookup").toBase58(),
              KEEPER_ORDER_MIN_BOUNTY_CREDIT: "1",
            },
          });
          let output = "";
          proc.stdout.on("data", (d) => (output += d));
          proc.stderr.on("data", (d) => (output += d));
          proc.on("error", reject);
          proc.on("exit", (code) => resolve({ code, output }));
        },
      );
      assert.equal(run.code, 0, run.output);
      assert.match(
        run.output,
        new RegExp(
          `shadow ${kind}: native trigger, settlement and bounty checks passed`,
        ),
      );
      assert.equal(simulations, 1);
      assert.ok(
        (svm.getAccount(c.order)?.lamports ?? 0n) > 0n,
        "shadow evaluation must not submit",
      );
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
