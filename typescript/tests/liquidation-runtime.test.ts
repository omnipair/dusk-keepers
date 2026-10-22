import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { spawn, execFileSync } from "node:child_process";
import {
  PublicKey,
  TransactionMessage,
  TransactionInstruction,
  VersionedTransaction,
  ComputeBudgetProgram,
} from "@solana/web3.js";
import bs58 from "bs58";
import { bank, planWithTable } from "./helpers/liquidation-bank.ts";
import { resolveLiquidationAccounts } from "../packages/dusk-adapter/src/liquidation.ts";
const layout = JSON.parse(
  readFileSync("protocol/keeper-account-layout.v1.json", "utf8"),
);
for (const [file, profile, expected] of [
  ["01-flash-BorrowPosition.json", "lending-bidder", "flash"],
  ["02-emergency-BorrowPosition.json", "lending-settler", "emergency"],
  // The final-notice case recovers on the next second's health check; the keeper
  // must clear its distress episode instead of submitting a stale flash fill.
  ["05-flash-LeveragePosition.json", "leverage", "observe"],
  // This separate case remains liquidatable after the observation commits.
  ["07-flash-LeveragePosition.json", "leverage", "flash"],
  ["03-emergency-LeveragePosition.json", "leverage", "emergency"],
] as const) {
  test(`native ${profile} completes discovery, observation, quote and simulation via RPC`, async () => {
    const f = JSON.parse(readFileSync(`fixtures/litesvm/${file}`, "utf8"));
    const svm = bank(f);
    const { lookupTable } = planWithTable(f, svm);
    const requests: string[] = [];
    let simulations = 0;
    const rpcAccount = (a: any) => {
      if (!a) return null;
      const field = (name: string) =>
        typeof a[name] === "function" ? a[name]() : a[name];
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
      try {
        const chunks = [];
        for await (const chunk of req) chunks.push(chunk);
        const call = JSON.parse(Buffer.concat(chunks).toString());
        const p = call.params;
        requests.push(call.method);
        let answer: any;
        const context = { slot: Number(svm.getClock().slot) };
        switch (call.method) {
          case "getSlot":
            answer = context.slot;
            break;
          case "getMultipleAccounts":
            answer = {
              context,
              value: p[0].map((key: string) =>
                rpcAccount(svm.getAccount(new PublicKey(key))),
              ),
            };
            break;
          case "getProgramAccounts": {
            const discriminator = p[1].filters[0].memcmp.bytes;
            answer = Object.keys(f.bank)
              .map((pubkey) => ({
                pubkey,
                account: rpcAccount(svm.getAccount(new PublicKey(pubkey))),
              }))
              .filter(
                (a) =>
                  a.account?.owner === p[0] &&
                  bs58.encode(
                    Buffer.from(a.account.data[0], "base64").subarray(0, 8),
                  ) === discriminator,
              );
            if (p[1].dataSlice)
              answer = answer.map((a: any) => ({
                ...a,
                account: { ...a.account, data: ["", "base64"] },
              }));
            break;
          }
          case "simulateTransaction": {
            simulations++;
            const tx = VersionedTransaction.deserialize(
              Buffer.from(p[0], "base64"),
            );
            const outcome: any = svm.simulateTransaction(tx);
            const failed = typeof outcome.err === "function";
            const meta = outcome.meta();
            const returned = meta.returnData();
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
                logs: meta.logs(),
                returnData: {
                  programId: new PublicKey(returned.programId()).toBase58(),
                  data: [
                    Buffer.from(returned.data()).toString("base64"),
                    "base64",
                  ],
                },
                accounts: (p[1].accounts?.addresses ?? []).map((key: string) =>
                  rpcAccount(
                    posts.has(key)
                      ? posts.get(key)
                      : svm.getAccount(new PublicKey(key)),
                  ),
                ),
              },
            };
            break;
          }
          default:
            throw new Error(`unexpected RPC ${call.method}`);
        }
        res.setHeader("content-type", "application/json");
        res.end(
          JSON.stringify({ jsonrpc: "2.0", id: call.id, result: answer }),
        );
      } catch (e) {
        res.end(
          JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            error: { message: String(e) },
          }),
        );
      }
    });
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const endpoint = `http://127.0.0.1:${(server.address() as any).port}`;
    const context = resolveLiquidationAccounts(
      layout,
      f.snapshot,
      f.position,
      f.buyer,
      f.kind,
      f.debtAsset,
    );
    async function once() {
      return await new Promise<{ code: number | null; output: string }>(
        (resolve, reject) => {
          const proc = spawn("target/debug/dusk-keeper", ["--once"], {
            env: {
              ...process.env,
              KEEPER_MODE: "shadow",
              KEEPER_PROFILE: profile,
              KEEPER_SHADOW_PUBLIC_KEY: f.buyer,
              PORT: "0",
              SOLANA_RPC_HTTP_URL: endpoint,
              KEEPER_LIQUIDATION_LOOKUP_TABLE: lookupTable,
              KEEPER_FLASH_SETTLEMENT: "inventory",
              KEEPER_FLASH_MARKET: context.named.market,
              KEEPER_FLASH_MAX_REPAY: "18446744073709551615",
              KEEPER_FLASH_MAX_PAYMENT_DEBIT: "18446744073709551615",
              KEEPER_FLASH_MIN_COLLATERAL_CREDIT:
                expected === "emergency" ? "18446744073709551615" : "1",
              KEEPER_LIQUIDATION_MIN_PROFIT: "0",
            },
          });
          let output = "";
          proc.stdout.on("data", (d) => (output += d));
          proc.stderr.on("data", (d) => (output += d));
          proc.on("error", reject);
          proc.on("exit", (code) => resolve({ code, output }));
        },
      );
    }
    try {
      const first = await once();
      assert.equal(first.code, 0, first.output);
      assert.match(
        first.output,
        /shadow observe|shadow flash|shadow emergency/,
      );
      const observe = JSON.parse(
        execFileSync("target/debug/dusk-keeper", ["--flash-plan"], {
          input: JSON.stringify({ ...f, action: "observe" }),
          encoding: "utf8",
        }),
      );
      const ixs = observe.instructions.map(
        (i: any) =>
          new TransactionInstruction({
            programId: new PublicKey(i.program_id),
            keys: i.accounts.map((a: any) => ({
              pubkey: new PublicKey(a.pubkey),
              isSigner: a.is_signer,
              isWritable: a.is_writable,
            })),
            data: Buffer.from(i.data),
          }),
      );
      const tx = new VersionedTransaction(
        new TransactionMessage({
          payerKey: new PublicKey(f.buyer),
          recentBlockhash: svm.latestBlockhash(),
          instructions: [
            ComputeBudgetProgram.requestHeapFrame({ bytes: 262144 }),
            ...ixs,
          ],
        }).compileToV0Message(),
      );
      const observed: any = svm.sendTransaction(tx);
      assert.ok(
        typeof observed.err !== "function",
        observed.meta?.().prettyLogs?.(),
      );
      const advanced = svm.getClock();
      advanced.slot += 1n;
      advanced.unixTimestamp += 1n;
      svm.setClock(advanced);
      const second = await once();
      assert.equal(second.code, 0, second.output);
      assert.match(second.output, new RegExp(`shadow ${expected}`));
      assert.ok(simulations >= 4);
      assert.ok(
        !requests.includes("sendTransaction"),
        "captured shadow must never submit",
      );
    } finally {
      await new Promise<void>((resolve, reject) =>
        server.close((e) => (e ? reject(e) : resolve())),
      );
    }
  });
}
