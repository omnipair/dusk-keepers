import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

const root = new URL("../", import.meta.url);
const contractPath = new URL("protocol/keeper-instructions.v1.json", root);
const fixturesPath = new URL("fixtures/conformance/v1/instruction-envelope-cases.json", root);

const actionDefinitions = [
  {
    action: "lending_liquidation_trigger",
    jobKind: "lending_liquidation_trigger",
    primary: ["dusk", "start_liquidation_auction"],
  },
  {
    action: "lending_liquidation_bid",
    jobKind: "lending_liquidation_bid",
    primary: ["dusk", "fill_liquidation_auction"],
  },
  {
    action: "lending_liquidation_floor_settle",
    jobKind: "lending_liquidation_settle",
    primary: ["dusk", "backstop_liquidation_auction"],
  },
  {
    action: "leverage_liquidation",
    jobKind: "leverage_liquidation",
    primary: ["dusk", "liquidate_leverage_position"],
  },
  {
    action: "delegated_close_take_profit",
    jobKind: "take_profit",
    primary: ["dusk", "delegated_close_leverage"],
    cpi: [
      ["leverage_delegate", "before_take_profit"],
      ["leverage_delegate", "after_close_order"],
    ],
    requiredOptionalAccounts: ["leverage_delegation", "delegated_program"],
  },
  {
    action: "delegated_close_stop_loss",
    jobKind: "stop_loss",
    primary: ["dusk", "delegated_close_leverage"],
    cpi: [
      ["leverage_delegate", "before_stop_loss"],
      ["leverage_delegate", "after_close_order"],
    ],
    requiredOptionalAccounts: ["leverage_delegation", "delegated_program"],
  },
  {
    action: "protocol_revenue_auction_settle",
    jobKind: "auction_bid",
    primary: ["dusk", "settle_protocol_auction"],
  },
  {
    action: "eligible_proposal_queue",
    jobKind: "lifecycle",
    primary: ["dusk", "queue_parameter_proposal"],
  },
  {
    action: "eligible_proposal_execute",
    jobKind: "lifecycle",
    primary: ["dusk", "execute_parameter_proposal"],
  },
];

async function readJson(path) {
  return JSON.parse(await readFile(new URL(path, root), "utf8"));
}

function instructionKey(program, instructionName) {
  return `${program}:${instructionName}`;
}

async function buildContract() {
  const lock = await readJson("protocol.lock.json");
  const programs = new Map();
  for (const pinned of lock.programs) {
    const idl = await readJson(pinned.idl.path);
    assert.equal(idl.address, pinned.programId, `${pinned.name}: IDL address differs from lock`);
    programs.set(pinned.name, { pinned, idl });
  }

  const requested = [];
  for (const definition of actionDefinitions) {
    requested.push(definition.primary, ...(definition.cpi ?? []));
  }
  const uniqueKeys = [...new Set(requested.map(([program, name]) => instructionKey(program, name)))];

  const instructions = uniqueKeys.map((key) => {
    const [program, instructionName] = key.split(":");
    const source = programs.get(program);
    assert.ok(source, `missing ${program} in protocol lock`);
    const instruction = source.idl.instructions.find((entry) => entry.name === instructionName);
    assert.ok(instruction, `${key}: instruction missing from pinned IDL`);
    assert.equal(instruction.discriminator.length, 8, `${key}: discriminator must be 8 bytes`);
    const anchorDiscriminator = createHash("sha256")
      .update(`global:${instructionName}`)
      .digest()
      .subarray(0, 8);
    assert.deepEqual(
      Buffer.from(instruction.discriminator),
      anchorDiscriminator,
      `${key}: IDL discriminator differs from Anchor global discriminator`,
    );
    for (const account of instruction.accounts) {
      assert.ok(!account.accounts, `${key}: nested account groups require explicit flattening`);
    }
    return {
      key,
      program,
      programId: source.pinned.programId,
      instructionName,
      discriminatorHex: Buffer.from(instruction.discriminator).toString("hex"),
      accounts: instruction.accounts.map((account) => ({
        name: account.name,
        writable: account.writable === true,
        signer: account.signer === true,
        optional: account.optional === true,
        fixedAddress: account.address ?? null,
      })),
    };
  });

  return {
    $schema: "./schemas/instruction-contract.schema.json",
    schemaVersion: 1,
    protocolRevision: lock.revision,
    programs: lock.programs.map((program) => ({
      name: program.name,
      programId: program.programId,
      idlPath: program.idl.path,
      idlSha256: program.idl.sha256,
    })),
    actions: actionDefinitions.map((definition) => ({
      action: definition.action,
      jobKind: definition.jobKind,
      primaryInstructionKey: instructionKey(...definition.primary),
      cpiInstructionKeys: (definition.cpi ?? []).map((parts) => instructionKey(...parts)),
      requiredOptionalAccounts: definition.requiredOptionalAccounts ?? [],
    })),
    instructions,
  };
}

function encodeU16(value) {
  const buffer = Buffer.alloc(2);
  buffer.writeUInt16LE(value);
  return buffer;
}

function encodeU32(value) {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32LE(value);
  return buffer;
}

function encodeU64(value) {
  const buffer = Buffer.alloc(8);
  buffer.writeBigUInt64LE(BigInt(value));
  return buffer;
}

function encodeVector(bytes) {
  return Buffer.concat([encodeU32(bytes.length), bytes]);
}

function discriminator(specification) {
  return Buffer.from(specification.discriminatorHex, "hex");
}

function directInstructionData(specification) {
  const prefix = discriminator(specification);
  switch (specification.instructionName) {
    case "start_liquidation_auction":
    case "queue_parameter_proposal":
    case "execute_parameter_proposal":
      return prefix;
    case "fill_liquidation_auction":
      return Buffer.concat([prefix, encodeU64(1_000), encodeU64(900)]);
    // One argument, not four: the backstop pays the caller a bounty and takes
    // its loss parameters from market state rather than from the caller.
    case "backstop_liquidation_auction":
      return Buffer.concat([prefix, encodeU64(100)]);
    case "liquidate_leverage_position":
      return Buffer.concat([prefix, Buffer.from([0])]);
    case "settle_protocol_auction":
      return Buffer.concat([prefix, Buffer.from([0, 0]), encodeU64(1_000), encodeU64(950)]);
    case "before_take_profit":
    case "before_stop_loss":
    case "after_close_order":
      return Buffer.concat([prefix, encodeU64(7)]);
    default:
      throw new Error(`${specification.key}: fixture encoder is intentionally unavailable`);
  }
}

function base58Encode(bytes) {
  const alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
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
  return result + digits.reverse().map((digit) => alphabet[digit]).join("");
}

function deterministicPubkey(seed) {
  return base58Encode(createHash("sha256").update(seed).digest());
}

function resolvedMeta(contract, action, specification, account) {
  const delegatedProgram = contract.programs.find((program) => program.name === "leverage_delegate");
  let address = account.fixedAddress;
  if (!address && account.name === "program") address = specification.programId;
  if (!address && account.name === "delegated_program") address = delegatedProgram.programId;
  if (!address) address = deterministicPubkey(`${action}:${specification.key}:${account.name}`);
  return {
    name: account.name,
    address,
    writable: account.writable,
    signer: account.signer,
  };
}

function resolvedAccounts(contract, actionContract, specification) {
  return specification.accounts
    .filter(
      (account) =>
        !account.optional || actionContract.requiredOptionalAccounts.includes(account.name),
    )
    .map((account) => resolvedMeta(contract, actionContract.action, specification, account));
}

function instructionEnvelope(contract, actionContract, specification, data) {
  return {
    specificationKey: specification.key,
    program: specification.program,
    programId: specification.programId,
    instructionName: specification.instructionName,
    dataHex: data.toString("hex"),
    accounts: resolvedAccounts(contract, actionContract, specification),
    remainingAccounts: [],
  };
}

function buildValidEnvelope(contract, actionContract, index) {
  const instructions = new Map(contract.instructions.map((entry) => [entry.key, entry]));
  const primarySpec = instructions.get(actionContract.primaryInstructionKey);
  const hooks = actionContract.cpiInstructionKeys.map((key) => {
    const specification = instructions.get(key);
    return instructionEnvelope(
      contract,
      actionContract,
      specification,
      directInstructionData(specification),
    );
  });

  let primaryData;
  if (primarySpec.instructionName === "delegated_close_leverage") {
    const before = Buffer.from(hooks[0].dataHex, "hex");
    const after = Buffer.from(hooks[1].dataHex, "hex");
    primaryData = Buffer.concat([
      discriminator(primarySpec),
      Buffer.from([0]),
      encodeU64(800),
      encodeVector(before),
      encodeVector(after),
      encodeU16(hooks[0].accounts.length),
    ]);
  } else {
    primaryData = directInstructionData(primarySpec);
  }

  const primary = instructionEnvelope(contract, actionContract, primarySpec, primaryData);
  if (hooks.length > 0) {
    primary.remainingAccounts = hooks.flatMap((hook) => structuredClone(hook.accounts));
  }
  const envelope = {
    schemaVersion: 1,
    intent: {
      schemaVersion: 1,
      intentId: `intent-${actionContract.action}`,
      protocolRevision: contract.protocolRevision,
      action: actionContract.action,
      jobKind: actionContract.jobKind,
      market: deterministicPubkey(`market:${actionContract.action}`),
      target: deterministicPubkey(`target:${actionContract.action}`),
      observedSlot: 1_000 + index,
      expectedStateHash: createHash("sha256")
        .update(`state:${actionContract.action}`)
        .digest("hex"),
    },
    primary,
    cpiHooks: hooks,
    paritySha256: "",
  };
  envelope.paritySha256 = envelopeParitySha256(envelope);
  return envelope;
}

function canonicalWriter() {
  const chunks = [];
  return {
    u8(value) {
      chunks.push(Buffer.from([value]));
    },
    u32(value) {
      chunks.push(encodeU32(value));
    },
    u64(value) {
      chunks.push(encodeU64(value));
    },
    string(value) {
      const bytes = Buffer.from(value, "utf8");
      this.u32(bytes.length);
      chunks.push(bytes);
    },
    bytes(bytes) {
      this.u32(bytes.length);
      chunks.push(bytes);
    },
    finish() {
      return Buffer.concat(chunks);
    },
  };
}

function writeMeta(writer, meta) {
  writer.string(meta.name);
  writer.string(meta.address);
  writer.u8(meta.writable ? 1 : 0);
  writer.u8(meta.signer ? 1 : 0);
}

function writeInstruction(writer, instruction) {
  writer.string(instruction.specificationKey);
  writer.string(instruction.program);
  writer.string(instruction.programId);
  writer.string(instruction.instructionName);
  writer.bytes(Buffer.from(instruction.dataHex, "hex"));
  writer.u32(instruction.accounts.length);
  for (const meta of instruction.accounts) writeMeta(writer, meta);
  writer.u32(instruction.remainingAccounts.length);
  for (const meta of instruction.remainingAccounts) writeMeta(writer, meta);
}

function envelopeParitySha256(envelope) {
  const writer = canonicalWriter();
  writer.u8(envelope.schemaVersion);
  writer.u8(envelope.intent.schemaVersion);
  writer.string(envelope.intent.intentId);
  writer.string(envelope.intent.protocolRevision);
  writer.string(envelope.intent.action);
  writer.string(envelope.intent.jobKind);
  writer.string(envelope.intent.market);
  writer.string(envelope.intent.target);
  writer.u64(envelope.intent.observedSlot);
  writer.string(envelope.intent.expectedStateHash);
  writeInstruction(writer, envelope.primary);
  writer.u32(envelope.cpiHooks.length);
  for (const hook of envelope.cpiHooks) writeInstruction(writer, hook);
  return createHash("sha256").update(writer.finish()).digest("hex");
}

function mutate(validCases, action, name, expectedError, mutation) {
  const source = validCases.find((entry) => entry.envelope.intent.action === action);
  assert.ok(source, `${name}: missing source action ${action}`);
  const envelope = structuredClone(source.envelope);
  mutation(envelope);
  return { name, expectedError, envelope };
}

function buildFixtures(contract) {
  const validCases = contract.actions.map((action, index) => ({
    name: action.action.replaceAll("_", " "),
    envelope: buildValidEnvelope(contract, action, index),
  }));

  const invalidCases = [
    mutate(validCases, "lending_liquidation_trigger", "revision mismatch", "revision_mismatch", (envelope) => {
      envelope.intent.protocolRevision = "another-revision";
    }),
    mutate(validCases, "lending_liquidation_trigger", "job kind mismatch", "job_kind_mismatch", (envelope) => {
      envelope.intent.jobKind = "take_profit";
    }),
    mutate(validCases, "lending_liquidation_trigger", "wrong instruction name", "instruction_mismatch", (envelope) => {
      envelope.primary.instructionName = "execute_parameter_proposal";
    }),
    mutate(validCases, "lending_liquidation_trigger", "wrong program id", "program_id_mismatch", (envelope) => {
      envelope.primary.programId = deterministicPubkey("wrong-program");
    }),
    mutate(validCases, "lending_liquidation_trigger", "invalid 32-byte pubkey", "invalid_pubkey", (envelope) => {
      envelope.primary.accounts[0].address = "22222222222222222222222222222222";
    }),
    mutate(validCases, "lending_liquidation_trigger", "bad discriminator", "discriminator_mismatch", (envelope) => {
      envelope.primary.dataHex = `00${envelope.primary.dataHex.slice(2)}`;
    }),
    mutate(validCases, "eligible_proposal_queue", "missing required account", "missing_required_account", (envelope) => {
      envelope.primary.accounts.splice(1, 1);
    }),
    mutate(validCases, "lending_liquidation_trigger", "required account order swapped", "account_order_mismatch", (envelope) => {
      [envelope.primary.accounts[0], envelope.primary.accounts[1]] = [
        envelope.primary.accounts[1],
        envelope.primary.accounts[0],
      ];
    }),
    mutate(validCases, "lending_liquidation_trigger", "account flags changed", "account_flags_mismatch", (envelope) => {
      envelope.primary.accounts[0].writable = false;
    }),
    mutate(validCases, "lending_liquidation_bid", "fixed token program changed", "fixed_address_mismatch", (envelope) => {
      envelope.primary.accounts.find((account) => account.name === "token_program").address =
        deterministicPubkey("wrong-token-program");
    }),
    mutate(validCases, "delegated_close_take_profit", "required delegation account omitted", "required_optional_account_missing", (envelope) => {
      envelope.primary.accounts = envelope.primary.accounts.filter(
        (account) => account.name !== "leverage_delegation",
      );
    }),
    mutate(validCases, "delegated_close_take_profit", "CPI hook sequence changed", "cpi_hook_mismatch", (envelope) => {
      envelope.cpiHooks.reverse();
    }),
    mutate(validCases, "delegated_close_stop_loss", "remaining accounts changed", "remaining_accounts_mismatch", (envelope) => {
      [envelope.primary.remainingAccounts[0], envelope.primary.remainingAccounts[1]] = [
        envelope.primary.remainingAccounts[1],
        envelope.primary.remainingAccounts[0],
      ];
    }),
    mutate(validCases, "delegated_close_stop_loss", "embedded hook bytes changed", "delegated_hook_data_mismatch", (envelope) => {
      envelope.cpiHooks[0].dataHex = `${envelope.cpiHooks[0].dataHex.slice(0, -2)}08`;
    }),
    mutate(validCases, "eligible_proposal_execute", "canonical parity changed", "parity_mismatch", (envelope) => {
      envelope.paritySha256 = "0".repeat(64);
    }),
  ];

  return {
    $schema: "../../../protocol/schemas/instruction-envelope-fixture.schema.json",
    schemaVersion: 1,
    validCases,
    invalidCases,
  };
}

function formatted(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

async function main() {
  const contract = await buildContract();
  const fixtures = buildFixtures(contract);
  if (process.argv.includes("--write")) {
    await writeFile(contractPath, formatted(contract));
    await writeFile(fixturesPath, formatted(fixtures));
    console.log(`wrote ${contract.instructions.length} instruction specifications and ${fixtures.validCases.length} valid fixtures`);
    return;
  }

  assert.equal(await readFile(contractPath, "utf8"), formatted(contract), "instruction contract is stale");
  assert.equal(await readFile(fixturesPath, "utf8"), formatted(fixtures), "instruction fixtures are stale");
  console.log(`verified ${contract.instructions.length} IDL-derived instruction specifications and ${fixtures.validCases.length + fixtures.invalidCases.length} envelope fixtures`);
}

await main();
