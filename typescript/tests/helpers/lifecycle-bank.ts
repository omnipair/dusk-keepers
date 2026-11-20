import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import anchor from "@coral-xyz/anchor";
import { IdlCoder } from "@coral-xyz/anchor/dist/cjs/coder/borsh/idl.js";
import {
  ComputeBudgetProgram,
  PublicKey,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import bs58 from "bs58";
import { bank, result } from "./liquidation-bank.ts";

const execute = promisify(execFile);
const idl = JSON.parse(readFileSync("artifacts/dusk.json", "utf8"));
const program = new PublicKey(idl.address);
const coder = new anchor.BorshInstructionCoder(idl);
const tableOwner = new PublicKey("AddressLookupTab1e1111111111111111111111111");
const key = (name: string) =>
  new PublicKey(createHash("sha256").update(name).digest());
const u64 = (value: bigint) => {
  const bytes = Buffer.alloc(8);
  bytes.writeBigUInt64LE(value);
  return bytes;
};
const pda = (...seeds: Buffer[]) =>
  PublicKey.findProgramAddressSync(seeds, program)[0];
const codec = (name: string) =>
  IdlCoder.typeDefLayout({
    typeDef: idl.types.find((entry: any) => entry.name === name),
    types: idl.types,
  });
type Change = "manager" | "recipient" | "handoff";

function fixture(change: Change, successor?: PublicKey) {
  const captured = JSON.parse(
    readFileSync("fixtures/litesvm/01-flash-BorrowPosition.json", "utf8"),
  );
  const svm = bank(captured);
  const known = new Set<string>(Object.keys(captured.bank));
  const discriminator = Buffer.from(
    idl.accounts.find((a: any) => a.name === "Market").discriminator,
  );
  const market = new PublicKey(
    Object.keys(captured.bank).find((address) =>
      Buffer.from(captured.bank[address].data)
        .subarray(0, 8)
        .equals(discriminator),
    )!,
  );
  const payer = new PublicKey(captured.buyer);
  const next = successor ?? key("lifecycle-test-successor");
  const recipient = key("lifecycle-test-recipient");
  const manager = key("lifecycle-test-manager");
  const put = (address: PublicKey, owner: PublicKey, data: Buffer) => {
    known.add(address.toBase58());
    svm.setAccount(address, {
      owner,
      data,
      lamports: 100_000_000,
      executable: false,
    });
  };
  const state = (name: string, address: PublicKey) =>
    codec(name).decode(Buffer.from(svm.getAccount(address)!.data).subarray(8));
  const updateMarket = (value: any) => {
    const bytes = Buffer.from(svm.getAccount(market)!.data);
    codec("Market").encode(value, bytes.subarray(8));
    put(market, program, bytes);
  };
  const initial = state("Market", market);
  initial.operator.authority = payer;
  initial.operator.pending = PublicKey.default;
  initial.operator.revenue_recipient = payer;
  updateMarket(initial);
  const proposal = pda(
    Buffer.from("operator_change"),
    market.toBuffer(),
    u64(BigInt(initial.operator.next_nonce.toString())),
  );
  const specification = idl.instructions.find(
    (ix: any) => ix.name === "announce_operator_change",
  );
  const supplied: Record<string, PublicKey> = {
    payer,
    operator: payer,
    market,
    proposal,
    event_authority: pda(Buffer.from("__event_authority")),
    program,
  };
  const announce = new TransactionInstruction({
    programId: program,
    keys: specification.accounts.map((a: any) => ({
      pubkey: supplied[a.name] ?? new PublicKey(a.address),
      isSigner: !!a.signer,
      isWritable: !!a.writable,
    })),
    data: coder.encode("announce_operator_change", {
      args: {
        nonce: initial.operator.next_nonce,
        expected_epoch: initial.operator.epoch,
        updates: [],
        next_operator: change === "handoff" ? next : null,
        next_manager: change === "manager" ? manager : null,
        revenue_recipient: change === "recipient" ? recipient : null,
        interest_share_bps: null,
        metadata: {
          version: 1,
          title: "Keeper lifecycle regression",
          description_uri: "https://example.com/keeper-test",
          description_sha256: Array(32).fill(7),
          description_len: 1,
        },
      },
    }),
  });
  // Let the pinned program create a valid notice, including its digest and deadlines.
  const transaction = new VersionedTransaction(
    new TransactionMessage({
      payerKey: payer,
      recentBlockhash: PublicKey.default.toBase58(),
      instructions: [
        ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }),
        announce,
      ],
    }).compileToV0Message(),
  );
  assert.equal(
    result(svm, transaction),
    null,
    "announcement executes in the captured bank",
  );
  known.add(proposal.toBase58());
  put(next, PublicKey.default, Buffer.alloc(0));
  const notice = state("OperatorProposal", proposal);
  const advance = (slot: bigint) => {
    const clock = svm.getClock();
    clock.slot = slot;
    svm.setClock(clock);
  };
  return {
    svm,
    known,
    market,
    payer,
    next,
    recipient,
    manager,
    proposal,
    notice,
    state,
    put,
    updateMarket,
    advance,
  };
}
type Fixture = ReturnType<typeof fixture>;

function installLookupTable(f: Fixture) {
  const address = key("lifecycle-test-lookup-table");
  const addresses = [f.market, f.proposal];
  const data = Buffer.alloc(56 + 32 * addresses.length);
  data.writeUInt32LE(1);
  data.writeBigUInt64LE(0xffffffffffffffffn, 4);
  addresses.forEach((value, index) =>
    value.toBuffer().copy(data, 56 + 32 * index),
  );
  f.put(address, tableOwner, data);
  return address;
}

function rpcAccount(account: any) {
  if (!account) return null;
  const field = (name: string) =>
    typeof account[name] === "function" ? account[name]() : account[name];
  if (Number(field("lamports")) === 0) return null;
  return {
    lamports: Number(field("lamports")),
    owner: new PublicKey(field("owner")).toBase58(),
    executable: field("executable"),
    data: [Buffer.from(field("data")).toString("base64"), "base64"],
  };
}

async function run(f: Fixture, signer: PublicKey, table?: PublicKey) {
  const requests: any[] = [];
  const simulations: {
    transaction: VersionedTransaction;
    error: string | null;
  }[] = [];
  const server = createServer(async (req, res) => {
    let id = 1;
    try {
      const chunks: Buffer[] = [];
      for await (const chunk of req) chunks.push(chunk);
      const call = JSON.parse(Buffer.concat(chunks).toString());
      id = call.id;
      requests.push(call);
      const p = call.params;
      const context = { slot: Number(f.svm.getClock().slot) };
      let answer: any;
      switch (call.method) {
        case "getSlot":
          answer = context.slot;
          break;
        case "getMultipleAccounts":
          answer = {
            context,
            value: p[0].map((address: string) =>
              rpcAccount(f.svm.getAccount(new PublicKey(address))),
            ),
          };
          break;
        case "getProgramAccounts":
          answer = [...f.known]
            .map((pubkey) => ({
              pubkey,
              account: rpcAccount(f.svm.getAccount(new PublicKey(pubkey))),
            }))
            .filter(
              (a) =>
                a.account?.owner === p[0] &&
                bs58.encode(
                  Buffer.from(a.account.data[0], "base64").subarray(0, 8),
                ) === p[1].filters[0].memcmp.bytes,
            );
          if (p[1].dataSlice)
            answer = answer.map((a: any) => ({
              ...a,
              account: { ...a.account, data: ["", "base64"] },
            }));
          break;
        case "simulateTransaction": {
          // Solana's independent decoder also rejects an inconsistent signature header.
          const transaction = VersionedTransaction.deserialize(
            Buffer.from(p[0], "base64"),
          );
          const outcome: any = f.svm.simulateTransaction(transaction);
          const failed = typeof outcome.err === "function";
          const meta = outcome.meta();
          const posts = new Map<string, any>(
            failed
              ? []
              : outcome
                  .postAccounts()
                  .map(([address, account]: any) => [
                    new PublicKey(address).toBase58(),
                    account,
                  ]),
          );
          const error = failed ? String(outcome.err()) : null;
          simulations.push({ transaction, error });
          answer = {
            context,
            value: {
              err: error,
              logs: meta.logs(),
              accounts: (p[1].accounts?.addresses ?? []).map(
                (address: string) =>
                  rpcAccount(
                    posts.get(address) ??
                      f.svm.getAccount(new PublicKey(address)),
                  ),
              ),
            },
          };
          break;
        }
        default:
          throw new Error(`unexpected RPC ${call.method}`);
      }
      res.end(JSON.stringify({ jsonrpc: "2.0", id, result: answer }));
    } catch (error) {
      res.end(
        JSON.stringify({
          jsonrpc: "2.0",
          id,
          error: { message: String(error) },
        }),
      );
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const { stdout, stderr } = await execute(
      "target/debug/dusk-keeper",
      ["--once"],
      {
        // Do not inherit a real signer, database, RPC, or unrelated keeper configuration.
        env: {
          PATH: process.env.PATH,
          KEEPER_PROFILE: "lifecycle",
          KEEPER_MODE: "shadow",
          KEEPER_SHADOW_PUBLIC_KEY: signer.toBase58(),
          SOLANA_RPC_HTTP_URL: `http://127.0.0.1:${(server.address() as any).port}`,
          PORT: "0",
          ...(table
            ? { KEEPER_LIQUIDATION_LOOKUP_TABLE: table.toBase58() }
            : {}),
        },
        timeout: 20_000,
      },
    );
    assert.ok(
      !requests.some((r) => r.method === "sendTransaction"),
      "shadow does not submit",
    );
    return { output: stdout + stderr, requests, simulations };
  } finally {
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

export {
  fixture,
  run,
  installLookupTable,
  codec,
  pda,
  u64,
  program,
  key,
  tableOwner,
};
