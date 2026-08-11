import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

const root = new URL("../", import.meta.url);
const manifestPath = new URL("protocol/keeper-account-resolution.v1.json", root);
const fixturesPath = new URL("fixtures/conformance/v1/adapter-codec-cases.json", root);
const PDA_MARKER = Buffer.from("ProgramDerivedAddress", "utf8");
const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const FIELD_MODULUS = (1n << 255n) - 19n;
const EDWARDS_D = mod(-121665n * modInverse(121666n));
const SQRT_MINUS_ONE = modPow(2n, (FIELD_MODULUS - 1n) / 4n);

const inputNames = new Map([
  ["market.base_side.asset_mint", "baseAssetMint"],
  ["market.quote_side.asset_mint", "quoteAssetMint"],
  ["market.params_hash", "paramsHash"],
  ["market", "market"],
  ["borrow_position.position_id", "positionId"],
  ["leverage_position.position_id", "positionId"],
  ["collateral_mint", "collateralMint"],
  ["leverage_position", "position"],
  ["order.position", "position"],
  ["order.owner", "owner"],
  ["args.order_id", "orderId"],
  ["order", "order"],
]);

async function readJson(path) {
  return JSON.parse(await readFile(new URL(path, root), "utf8"));
}

function definedName(type) {
  if (!type || typeof type !== "object" || !("defined" in type)) return null;
  return typeof type.defined === "string" ? type.defined : type.defined.name;
}

function descendType(idl, type, path) {
  let current = type;
  for (const fieldName of path) {
    const name = definedName(current);
    assert.ok(name, `cannot traverse ${fieldName} through non-struct IDL type`);
    const definition = idl.types.find((entry) => entry.name === name);
    assert.equal(definition?.type.kind, "struct", `${name}: seed path requires a struct`);
    const field = definition.type.fields.find((entry) => entry.name === fieldName);
    assert.ok(field, `${name}.${fieldName}: seed field is absent from IDL`);
    current = field.type;
  }
  return current;
}

function seedEncoding(idl, instruction, seed) {
  let type;
  if (seed.kind === "account") {
    const [rootAccount, ...fieldPath] = seed.path.split(".");
    assert.ok(
      instruction.accounts.some((account) => account.name === rootAccount),
      `${instruction.name}:${seed.path}: seed account is absent`,
    );
    type = seed.account
      ? descendType(idl, { defined: seed.account }, fieldPath)
      : "pubkey";
  } else if (seed.kind === "arg") {
    const [argumentName, ...fieldPath] = seed.path.split(".");
    const argument = instruction.args.find((entry) => entry.name === argumentName);
    assert.ok(argument, `${instruction.name}:${seed.path}: seed argument is absent`);
    type = descendType(idl, argument.type, fieldPath);
  } else {
    throw new Error(`${instruction.name}: unsupported IDL seed kind ${seed.kind}`);
  }

  if (type === "pubkey") return "pubkey";
  if (type === "u64") return "u64_le";
  if (
    typeof type === "object" &&
    Array.isArray(type.array) &&
    type.array[0] === "u8" &&
    type.array[1] === 32
  ) {
    return "bytes32";
  }
  throw new Error(`${instruction.name}:${seed.path}: unsupported seed type ${JSON.stringify(type)}`);
}

function normalizeSeed(idl, instruction, seed) {
  if (seed.kind === "const") {
    const bytes = Buffer.from(seed.value);
    assert.ok(bytes.length > 0 && bytes.length <= 32, `${instruction.name}: invalid const seed`);
    return { kind: "const", valueHex: bytes.toString("hex") };
  }
  const name = inputNames.get(seed.path);
  assert.ok(name, `${instruction.name}:${seed.path}: seed input needs an explicit stable name`);
  return { kind: "input", name, encoding: seedEncoding(idl, instruction, seed) };
}

function rawSeedPath(seed) {
  return seed.kind === "const"
    ? `const:${Buffer.from(seed.value).toString("hex")}`
    : `${seed.kind}:${seed.path}`;
}

function buildResolutionManifest(lock, contract, idls) {
  const staticAccounts = [];
  const recipes = new Map();

  for (const specification of contract.instructions) {
    const idl = idls.get(specification.program);
    const instruction = idl.instructions.find(
      (entry) => entry.name === specification.instructionName,
    );
    assert.ok(instruction, `${specification.key}: instruction missing from pinned IDL`);

    for (const account of instruction.accounts) {
      let staticAddress = account.address ?? null;
      let staticSource = account.address ? "idl_fixed_address" : null;
      if (!staticAddress && account.name === "program") {
        staticAddress = specification.programId;
        staticSource = "instruction_program";
      }
      if (!staticAddress && account.name === "delegated_program") {
        const cpiPrograms = new Set(
          contract.actions
            .filter((action) => action.primaryInstructionKey === specification.key)
            .flatMap((action) => action.cpiInstructionKeys)
            .map((key) => contract.instructions.find((entry) => entry.key === key)?.programId),
        );
        cpiPrograms.delete(undefined);
        assert.equal(cpiPrograms.size, 1, `${specification.key}: ambiguous delegated program`);
        staticAddress = [...cpiPrograms][0];
        staticSource = "action_cpi_program";
      }
      if (staticAddress) {
        staticAccounts.push({
          instructionKey: specification.key,
          accountName: account.name,
          address: staticAddress,
          source: staticSource,
        });
      }

      if (!account.pda) continue;
      const key = `${specification.program}:${account.name}`;
      const normalized = account.pda.seeds.map((seed) => normalizeSeed(idl, instruction, seed));
      const usage = {
        instructionKey: specification.key,
        accountName: account.name,
        idlSeedPaths: account.pda.seeds.map(rawSeedPath),
      };
      const existing = recipes.get(key);
      if (existing) {
        assert.deepEqual(existing.seeds, normalized, `${key}: inconsistent IDL PDA recipe`);
        existing.usages.push(usage);
      } else {
        recipes.set(key, {
          key,
          program: specification.program,
          programId: specification.programId,
          usages: [usage],
          seeds: normalized,
        });
      }
    }
  }

  return {
    $schema: "./schemas/account-resolution-manifest.schema.json",
    schemaVersion: 1,
    protocolRevision: lock.revision,
    programs: lock.programs.map((program) => ({
      name: program.name,
      programId: program.programId,
      idlSha256: program.idl.sha256,
    })),
    staticAccounts,
    pdaRecipes: [...recipes.values()],
  };
}

function encodeU16(value) {
  const output = Buffer.alloc(2);
  output.writeUInt16LE(value);
  return output;
}

function encodeU32(value) {
  const output = Buffer.alloc(4);
  output.writeUInt32LE(value);
  return output;
}

function encodeU64(value) {
  const output = Buffer.alloc(8);
  output.writeBigUInt64LE(BigInt(value));
  return output;
}

function encodeVector(bytes) {
  return Buffer.concat([encodeU32(bytes.length), bytes]);
}

function wireToIdlArguments(specificationKey, argumentsValue) {
  switch (specificationKey) {
    case "dusk:trigger_liquidation_auction":
    case "dusk:queue_parameter_proposal":
    case "dusk:execute_parameter_proposal":
      return {};
    case "dusk:bid_liquidation_auction":
      return {
        args: {
          repay_amount: argumentsValue.repayAmount,
          min_collateral_out: argumentsValue.minCollateralOut,
        },
      };
    case "dusk:settle_liquidation_auction_floor":
      return {
        args: {
          repay_amount: argumentsValue.repayAmount,
          min_collateral_out: argumentsValue.minCollateralOut,
          max_insurance_draw: argumentsValue.maxInsuranceDraw,
          max_socialized_loss: argumentsValue.maxSocializedLoss,
        },
      };
    case "dusk:liquidate_leverage":
      return { args: { debt_asset: argumentsValue.debtAsset } };
    case "dusk:delegated_close_leverage":
      return {
        args: {
          debt_asset: argumentsValue.debtAsset,
          min_amount_out: argumentsValue.minAmountOut,
          delegated: {
            before_ix_data: Buffer.from(argumentsValue.beforeIxDataHex, "hex"),
            after_ix_data: Buffer.from(argumentsValue.afterIxDataHex, "hex"),
            before_accounts_len: argumentsValue.beforeAccountsLen,
          },
        },
      };
    case "leverage_delegate:before_take_profit":
    case "leverage_delegate:before_stop_loss":
    case "leverage_delegate:after_close_order":
      return { args: { order_id: argumentsValue.orderId } };
    case "dusk:settle_protocol_auction":
      return {
        args: {
          lane: { fee: "Fee", buyback: "Buyback" }[argumentsValue.lane],
          source: { swap: "Swap", interest: "Interest" }[argumentsValue.source],
          sold_amount: argumentsValue.soldAmount,
          max_payment_amount: argumentsValue.maxPaymentAmount,
        },
      };
    default:
      throw new Error(`${specificationKey}: reference encoder is unavailable`);
  }
}

function encodeIdlType(idl, type, value, path) {
  if (typeof type === "string") {
    if (type === "u8") {
      assert.ok(Number.isInteger(value) && value >= 0 && value <= 0xff, `${path}: invalid u8`);
      return Buffer.from([value]);
    }
    if (type === "u16") {
      assert.ok(Number.isInteger(value) && value >= 0 && value <= 0xffff, `${path}: invalid u16`);
      return encodeU16(value);
    }
    if (type === "u32") {
      assert.ok(Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff, `${path}: invalid u32`);
      return encodeU32(value);
    }
    if (type === "u64") return encodeU64(value);
    if (type === "bytes") {
      assert.ok(Buffer.isBuffer(value), `${path}: IDL bytes value must be a Buffer`);
      return encodeVector(value);
    }
    throw new Error(`${path}: unsupported golden-vector IDL scalar ${type}`);
  }

  const name = definedName(type);
  assert.ok(name, `${path}: unsupported golden-vector IDL type ${JSON.stringify(type)}`);
  const definition = idl.types.find((entry) => entry.name === name);
  assert.ok(definition, `${path}: missing IDL type ${name}`);
  if (definition.type.kind === "struct") {
    assert.ok(value && typeof value === "object", `${path}: struct value must be an object`);
    assert.deepEqual(
      Object.keys(value).sort(),
      definition.type.fields.map((field) => field.name).sort(),
      `${path}: struct fields differ from pinned IDL`,
    );
    return Buffer.concat(
      definition.type.fields.map((field) =>
        encodeIdlType(idl, field.type, value[field.name], `${path}.${field.name}`),
      ),
    );
  }
  if (definition.type.kind === "enum") {
    const ordinal = definition.type.variants.findIndex((variant) => variant.name === value);
    assert.notEqual(ordinal, -1, `${path}: enum variant differs from pinned IDL`);
    assert.ok(!definition.type.variants[ordinal].fields, `${path}: data enums are unsupported`);
    assert.ok(ordinal <= 0xff, `${path}: enum ordinal exceeds one byte`);
    return Buffer.from([ordinal]);
  }
  throw new Error(`${path}: unsupported IDL definition kind ${definition.type.kind}`);
}

function encodeIdlArguments(idl, instruction, values) {
  assert.deepEqual(
    Object.keys(values).sort(),
    instruction.args.map((argument) => argument.name).sort(),
    `${instruction.name}: argument names differ from pinned IDL`,
  );
  return Buffer.concat(
    instruction.args.map((argument) =>
      encodeIdlType(idl, argument.type, values[argument.name], `${instruction.name}.${argument.name}`),
    ),
  );
}

function normalizeIdlType(idl, type, visited = new Set()) {
  if (typeof type === "string") return type;
  const name = definedName(type);
  if (!name) return type;
  assert.ok(!visited.has(name), `${name}: recursive instruction argument type is unsupported`);
  const definition = idl.types.find((entry) => entry.name === name);
  assert.ok(definition, `${name}: missing IDL type`);
  const nextVisited = new Set(visited).add(name);
  if (definition.type.kind === "struct") {
    return {
      struct: definition.type.fields.map((field) => ({
        name: field.name,
        type: normalizeIdlType(idl, field.type, nextVisited),
      })),
    };
  }
  if (definition.type.kind === "enum") {
    return {
      enum: definition.type.variants.map((variant) => ({
        name: variant.name,
        fields: variant.fields ?? null,
      })),
    };
  }
  throw new Error(`${name}: unsupported IDL argument definition kind ${definition.type.kind}`);
}

function argumentLayoutSha256(idl, instruction) {
  const layout = instruction.args.map((argument) => ({
    name: argument.name,
    type: normalizeIdlType(idl, argument.type),
  }));
  return createHash("sha256").update(JSON.stringify(layout)).digest("hex");
}

function referenceInstructionData(contract, idls, specificationKey, argumentsValue) {
  const specification = contract.instructions.find((entry) => entry.key === specificationKey);
  assert.ok(specification, `${specificationKey}: specification missing from contract`);
  const idl = idls.get(specification.program);
  const instruction = idl.instructions.find(
    (entry) => entry.name === specification.instructionName,
  );
  assert.ok(instruction, `${specificationKey}: instruction missing from pinned IDL`);
  return Buffer.concat([
    Buffer.from(specification.discriminatorHex, "hex"),
    encodeIdlArguments(
      idl,
      instruction,
      wireToIdlArguments(specificationKey, argumentsValue),
    ),
  ]);
}

function buildEncodingCases(contract, idls) {
  const hookArguments = {
    "leverage_delegate:before_take_profit": { orderId: "72623859790382856" },
    "leverage_delegate:before_stop_loss": { orderId: "18446744073709551615" },
    "leverage_delegate:after_close_order": { orderId: "7" },
  };
  const beforeData = referenceInstructionData(
    contract,
    idls,
    "leverage_delegate:before_take_profit",
    hookArguments["leverage_delegate:before_take_profit"],
  ).toString("hex");
  const afterData = referenceInstructionData(
    contract,
    idls,
    "leverage_delegate:after_close_order",
    hookArguments["leverage_delegate:after_close_order"],
  ).toString("hex");

  const argumentsByKey = {
    "dusk:trigger_liquidation_auction": {},
    "dusk:bid_liquidation_auction": {
      repayAmount: "18446744073709551615",
      minCollateralOut: "0",
    },
    "dusk:settle_liquidation_auction_floor": {
      repayAmount: "1000",
      minCollateralOut: "850",
      maxInsuranceDraw: "100",
      maxSocializedLoss: "50",
    },
    "dusk:liquidate_leverage": { debtAsset: 1 },
    "dusk:delegated_close_leverage": {
      debtAsset: 0,
      minAmountOut: "800",
      beforeIxDataHex: beforeData,
      afterIxDataHex: afterData,
      beforeAccountsLen: 8,
    },
    ...hookArguments,
    "dusk:settle_protocol_auction": {
      lane: "buyback",
      source: "interest",
      soldAmount: "1000",
      maxPaymentAmount: "950",
    },
    "dusk:queue_parameter_proposal": {},
    "dusk:execute_parameter_proposal": {},
  };

  return contract.instructions.map((specification) => {
    const argumentsValue = argumentsByKey[specification.key];
    assert.ok(argumentsValue, `${specification.key}: missing golden argument vector`);
    const idl = idls.get(specification.program);
    const instruction = idl.instructions.find(
      (entry) => entry.name === specification.instructionName,
    );
    assert.ok(instruction, `${specification.key}: instruction missing from pinned IDL`);
    return {
      name: specification.key.replace(":", " "),
      specificationKey: specification.key,
      arguments: argumentsValue,
      argumentLayoutSha256: argumentLayoutSha256(idl, instruction),
      expectedDataHex: referenceInstructionData(
        contract,
        idls,
        specification.key,
        argumentsValue,
      ).toString("hex"),
    };
  });
}

function base58Encode(bytes) {
  const digits = [0];
  for (const byte of bytes) {
    let carry = byte;
    for (let index = 0; index < digits.length; index += 1) {
      const value = digits[index] * 256 + carry;
      digits[index] = value % 58;
      carry = Math.floor(value / 58);
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = Math.floor(carry / 58);
    }
  }
  let result = "";
  for (let index = 0; index < bytes.length - 1 && bytes[index] === 0; index += 1) result += "1";
  return result + digits.reverse().map((digit) => BASE58_ALPHABET[digit]).join("");
}

function base58Decode(value) {
  let leadingZeros = 0;
  while (leadingZeros < value.length && value[leadingZeros] === "1") leadingZeros += 1;
  const bytes = [];
  for (const character of value.slice(leadingZeros)) {
    const digit = BASE58_ALPHABET.indexOf(character);
    assert.notEqual(digit, -1, `${value}: invalid base58`);
    let carry = digit;
    for (let index = 0; index < bytes.length; index += 1) {
      const next = bytes[index] * 58 + carry;
      bytes[index] = next & 0xff;
      carry = next >> 8;
    }
    while (carry > 0) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }
  return Buffer.concat([Buffer.alloc(leadingZeros), Buffer.from(bytes.reverse())]);
}

function mod(value) {
  const reduced = value % FIELD_MODULUS;
  return reduced < 0n ? reduced + FIELD_MODULUS : reduced;
}

function modPow(base, exponent) {
  let result = 1n;
  let factor = mod(base);
  let remaining = exponent;
  while (remaining > 0n) {
    if ((remaining & 1n) === 1n) result = mod(result * factor);
    factor = mod(factor * factor);
    remaining >>= 1n;
  }
  return result;
}

function modInverse(value) {
  return modPow(value, FIELD_MODULUS - 2n);
}

function littleEndianBigInt(bytes) {
  let value = 0n;
  for (let index = bytes.length - 1; index >= 0; index -= 1) {
    value = (value << 8n) | BigInt(bytes[index]);
  }
  return value;
}

function isEd25519Point(bytes) {
  const y = mod(littleEndianBigInt(bytes) & ((1n << 255n) - 1n));
  const ySquared = mod(y * y);
  const numerator = mod(ySquared - 1n);
  const denominator = mod(EDWARDS_D * ySquared + 1n);
  if (denominator === 0n) return numerator === 0n;
  const ratio = mod(numerator * modInverse(denominator));
  let root = modPow(ratio, (FIELD_MODULUS + 3n) / 8n);
  if (mod(root * root) !== ratio) root = mod(root * SQRT_MINUS_ONE);
  return mod(root * root) === ratio;
}

function findProgramAddress(seeds, programId) {
  const program = base58Decode(programId);
  assert.equal(program.length, 32, `${programId}: program ID must decode to 32 bytes`);
  assert.ok(seeds.length < 16, "PDA recipe leaves no room for a bump seed");
  for (const seed of seeds) assert.ok(seed.length <= 32, "PDA seed exceeds 32 bytes");

  for (let bump = 255; bump >= 0; bump -= 1) {
    const digest = createHash("sha256")
      .update(Buffer.concat([...seeds, Buffer.from([bump]), program, PDA_MARKER]))
      .digest();
    if (!isEd25519Point(digest)) return { address: base58Encode(digest), bump };
  }
  throw new Error("unable to find an off-curve PDA");
}

function deterministicPubkey(seed) {
  return base58Encode(createHash("sha256").update(seed).digest());
}

function fixtureInput(recipeKey, seed) {
  switch (seed.encoding) {
    case "pubkey":
      return deterministicPubkey(`${recipeKey}:${seed.name}`);
    case "bytes32":
      return createHash("sha256").update(`${recipeKey}:${seed.name}`).digest("hex");
    case "u64_le":
      return "72623859790382856";
    default:
      throw new Error(`${recipeKey}:${seed.name}: unsupported fixture encoding`);
  }
}

function encodedSeed(seed, inputs) {
  if (seed.kind === "const") return Buffer.from(seed.valueHex, "hex");
  const value = inputs[seed.name];
  switch (seed.encoding) {
    case "pubkey": {
      const bytes = base58Decode(value);
      assert.equal(bytes.length, 32);
      return bytes;
    }
    case "bytes32": {
      const bytes = Buffer.from(value, "hex");
      assert.equal(bytes.length, 32);
      return bytes;
    }
    case "u64_le":
      return encodeU64(value);
    default:
      throw new Error(`unsupported PDA seed encoding ${seed.encoding}`);
  }
}

function buildPdaCases(manifest) {
  return manifest.pdaRecipes.map((recipe) => {
    const inputs = Object.fromEntries(
      recipe.seeds
        .filter((seed) => seed.kind === "input")
        .map((seed) => [seed.name, fixtureInput(recipe.key, seed)]),
    );
    const result = findProgramAddress(
      recipe.seeds.map((seed) => encodedSeed(seed, inputs)),
      recipe.programId,
    );
    return {
      name: recipe.key.replace(":", " "),
      recipeKey: recipe.key,
      inputs,
      expectedAddress: result.address,
      expectedBump: result.bump,
    };
  });
}

function buildFixtureBundle(contract, manifest, idls) {
  const pdaCases = buildPdaCases(manifest);
  const market = pdaCases.find((entry) => entry.recipeKey === "dusk:market");
  const eventAuthority = pdaCases.find(
    (entry) => entry.recipeKey === "dusk:event_authority",
  );
  const order = pdaCases.find((entry) => entry.recipeKey === "leverage_delegate:order");
  assert.ok(market && eventAuthority && order, "required negative-test PDA recipes are absent");

  return {
    $schema: "../../../protocol/schemas/adapter-codec-fixture.schema.json",
    schemaVersion: 1,
    encodingCases: buildEncodingCases(contract, idls),
    invalidEncodingCases: [
      {
        name: "unknown instruction",
        specificationKey: "dusk:not_a_keeper_instruction",
        arguments: {},
        expectedError: "unsupported_instruction",
      },
      {
        name: "u64 overflow",
        specificationKey: "dusk:bid_liquidation_auction",
        arguments: { repayAmount: "18446744073709551616", minCollateralOut: "0" },
        expectedError: "invalid_arguments",
      },
      {
        name: "negative u64",
        specificationKey: "dusk:bid_liquidation_auction",
        arguments: { repayAmount: "-1", minCollateralOut: "0" },
        expectedError: "invalid_arguments",
      },
      {
        name: "non-canonical u64",
        specificationKey: "dusk:bid_liquidation_auction",
        arguments: { repayAmount: "+1", minCollateralOut: "0" },
        expectedError: "invalid_arguments",
      },
      {
        name: "u8 overflow",
        specificationKey: "dusk:liquidate_leverage",
        arguments: { debtAsset: 256 },
        expectedError: "invalid_arguments",
      },
      {
        name: "unknown protocol auction enum",
        specificationKey: "dusk:settle_protocol_auction",
        arguments: {
          lane: "treasury",
          source: "interest",
          soldAmount: "1",
          maxPaymentAmount: "1",
        },
        expectedError: "invalid_arguments",
      },
      {
        name: "malformed delegated hook hex",
        specificationKey: "dusk:delegated_close_leverage",
        arguments: {
          debtAsset: 0,
          minAmountOut: "1",
          beforeIxDataHex: "abc",
          afterIxDataHex: "00",
          beforeAccountsLen: 1,
        },
        expectedError: "invalid_arguments",
      },
      {
        name: "unexpected no-argument field",
        specificationKey: "dusk:queue_parameter_proposal",
        arguments: { invented: true },
        expectedError: "invalid_arguments",
      },
    ],
    pdaCases,
    invalidPdaCases: [
      {
        name: "unknown PDA recipe",
        recipeKey: "dusk:unknown",
        inputs: {},
        expectedError: "unknown_recipe",
      },
      {
        name: "missing PDA input",
        recipeKey: market.recipeKey,
        inputs: Object.fromEntries(Object.entries(market.inputs).slice(1)),
        expectedError: "missing_input",
      },
      {
        name: "unexpected PDA input",
        recipeKey: eventAuthority.recipeKey,
        inputs: { invented: "1" },
        expectedError: "unexpected_input",
      },
      {
        name: "invalid pubkey PDA input",
        recipeKey: market.recipeKey,
        inputs: { ...market.inputs, baseAssetMint: "not-base58" },
        expectedError: "invalid_seed_value",
      },
      {
        name: "invalid bytes32 PDA input",
        recipeKey: market.recipeKey,
        inputs: { ...market.inputs, paramsHash: "00" },
        expectedError: "invalid_seed_value",
      },
      {
        name: "overflowing u64 PDA input",
        recipeKey: order.recipeKey,
        inputs: { ...order.inputs, orderId: "18446744073709551616" },
        expectedError: "invalid_seed_value",
      },
      {
        name: "non-canonical u64 PDA input",
        recipeKey: order.recipeKey,
        inputs: { ...order.inputs, orderId: "+1" },
        expectedError: "invalid_seed_value",
      },
    ],
    staticAccountCases: manifest.staticAccounts.map((account) => ({
      name: `${account.instructionKey} ${account.accountName}`,
      instructionKey: account.instructionKey,
      accountName: account.accountName,
      expectedAddress: account.address,
    })),
  };
}

function formatted(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

async function main() {
  const lock = await readJson("protocol.lock.json");
  const contract = await readJson("protocol/keeper-instructions.v1.json");
  const idls = new Map();
  for (const program of lock.programs) idls.set(program.name, await readJson(program.idl.path));

  const manifest = buildResolutionManifest(lock, contract, idls);
  const fixtures = buildFixtureBundle(contract, manifest, idls);
  if (process.argv.includes("--write")) {
    await writeFile(manifestPath, formatted(manifest));
    await writeFile(fixturesPath, formatted(fixtures));
    console.log(
      `wrote ${fixtures.encodingCases.length} encoding, ${fixtures.pdaCases.length} PDA, and ${fixtures.staticAccountCases.length} static-account vectors`,
    );
    return;
  }

  assert.equal(await readFile(manifestPath, "utf8"), formatted(manifest), "account resolution manifest is stale");
  assert.equal(await readFile(fixturesPath, "utf8"), formatted(fixtures), "adapter codec fixtures are stale");
  console.log(
    `verified ${fixtures.encodingCases.length} encoding, ${fixtures.pdaCases.length} PDA, and ${fixtures.staticAccountCases.length} static-account vectors`,
  );
}

await main();
