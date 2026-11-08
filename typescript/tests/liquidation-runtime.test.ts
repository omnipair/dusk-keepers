import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import {
  PublicKey,
  VersionedTransaction,
} from "@solana/web3.js";
import bs58 from "bs58";
import { createHash } from "node:crypto";
import { withOracle, base as oracleFeed, provider as oracleProvider } from "./helpers/oracle-fixture.ts";
import { bank, planWithTable } from "./helpers/liquidation-bank.ts";
import { resolveLiquidationAccounts } from "../packages/dusk-adapter/src/liquidation.ts";
const layout = JSON.parse(
  readFileSync("protocol/keeper-account-layout.v1.json", "utf8"),
);
for (const { file, profile, expected, settlement, maxRepay, minProfit, name, mode = "both", rejectOrdinary = false, rejectEmergencyDust = false, emergencySlippage = "0", driftEmergencyQuote = false, oracle = false, devnetAllMarkets = false, maxSends = "4", debtShares = null, clockMismatch = false } of [
  {
    file: "01-flash-BorrowPosition.json", profile: "lending", expected: "flash",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "0",
    name: "devnet evaluates a market outside the obsolete selector", devnetAllMarkets: true,
  },
  {
    file: "01-flash-BorrowPosition.json", profile: "lending", expected: "monitor",
    settlement: "inventory", maxRepay: "1", minProfit: "0",
    name: "exhausted transaction budget reports health without planning execution", maxSends: "0",
  },
  {
    file: "01-flash-BorrowPosition.json", profile: "lending", expected: "healthy",
    settlement: "inventory", maxRepay: "1", minProfit: "0",
    name: "healthy positions recover a transient clock mismatch without liquidation probes", debtShares: 1n, clockMismatch: true,
  },
  {
    file: "01-flash-BorrowPosition.json", profile: "lending", expected: "debt_free",
    settlement: "inventory", maxRepay: "1", minProfit: "0",
    name: "debt-free positions show collateral without inventing health ratios", debtShares: 0n,
  },
  {
    file: "01-flash-BorrowPosition.json",
    profile: "lending",
    expected: "flash",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "inventory flash",
  },
  {
    file: "02-emergency-BorrowPosition.json",
    profile: "lending",
    expected: "emergency",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "emergency settlement",
  },
  {
    file: "02-emergency-BorrowPosition.json",
    profile: "lending",
    mode: "emergency",
    expected: "emergency",
    settlement: "inventory",
    maxRepay: "not-required",
    minProfit: "0",
    name: "emergency-only skips ordinary evaluation",
  },
  {
    file: "02-emergency-BorrowPosition.json",
    profile: "lending",
    mode: "ordinary",
    expected: "unfilled",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "ordinary-only never falls back",
  },
  {
    file: "02-emergency-BorrowPosition.json",
    profile: "lending",
    expected: "error",
    rejectOrdinary: true,
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "unexpected ordinary failure never falls back",
  },
  {
    file: "01-flash-BorrowPosition.json", profile: "lending", expected: "flash",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "0",
    name: "oracle-backed inventory flash", oracle: true,
  },
  // Final-notice eligibility is rechecked directly on every pass.
  {
    file: "05-flash-LeveragePosition.json",
    profile: "leverage",
    expected: "flash",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "final-notice leverage",
  },
  // No registration is needed for a fresh leverage fill.
  {
    file: "07-flash-LeveragePosition.json",
    profile: "leverage",
    expected: "flash",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "fresh leverage",
  },
  {
    file: "03-emergency-LeveragePosition.json",
    profile: "leverage",
    expected: "emergency",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    name: "leverage emergency",
  },
  {
    file: "03-emergency-LeveragePosition.json",
    profile: "leverage",
    expected: "emergency",
    settlement: "inventory",
    maxRepay: "18446744073709551615",
    minProfit: "0",
    rejectEmergencyDust: true,
    name: "zero-output dust probe still reaches a valid full emergency liquidation",
  },
  {
    file: "02-emergency-BorrowPosition.json", profile: "lending", expected: "unfilled",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "0",
    driftEmergencyQuote: true, name: "default zero slippage rejects a worse emergency quote",
  },
  {
    file: "02-emergency-BorrowPosition.json", profile: "lending", expected: "emergency",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "0",
    driftEmergencyQuote: true, emergencySlippage: "10", name: "explicit tolerance accepts small emergency quote movement",
  },
  {
    file: "02-emergency-BorrowPosition.json", profile: "lending", expected: "unfilled",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "96",
    driftEmergencyQuote: true, emergencySlippage: "10", name: "emergency tolerance preserves the configured profit floor",
  },
  {
    file: "02-emergency-BorrowPosition.json", profile: "lending", expected: "emergency",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "1",
    driftEmergencyQuote: true, emergencySlippage: "10000", name: "100 percent emergency tolerance accepts execution",
  },
  {
    file: "02-emergency-BorrowPosition.json", profile: "lending", expected: "unfilled",
    settlement: "inventory", maxRepay: "18446744073709551615", minProfit: "96",
    driftEmergencyQuote: true, emergencySlippage: "10000", name: "100 percent emergency tolerance still enforces minimum profit",
  },
  // The geometric probes top out at seven atoms for this fixture. Adaptive
  // refinement must find the non-geometric 996-atom fill yielding eight.
  {
    file: "01-flash-BorrowPosition.json",
    profile: "lending",
    expected: "flash",
    settlement: "dusk",
    maxRepay: "1000",
    minProfit: "8",
    name: "adaptive non-geometric partial",
  },
] as const) {
  test(`native ${profile} completes ${name} via RPC`, async () => {
    const f = JSON.parse(readFileSync(`fixtures/litesvm/${file}`, "utf8"));
    const oracleState = oracle ? withOracle(f).state : null;
    const svm = bank(f);
    if (debtShares !== null) {
      const key = new PublicKey(f.position);
      const account = svm.getAccount(key)!;
      const data = Buffer.from(account.data);
      const fields = layout.accounts.find((a: any) => a.name === f.kind).fields;
      for (const path of ["fixed_base_shares", "fixed_quote_shares"]) {
        const field = fields.find((f: any) => f.path === path);
        data.fill(0, field.offset, field.offset + 16);
        if (path === (f.debtAsset === 0 ? "fixed_base_shares" : "fixed_quote_shares")) data.writeBigUInt64LE(debtShares, field.offset);
      }
      svm.setAccount(key, { ...account, data });
    }
    if (oracleState) {
      const clock = svm.getClock();
      const data = Buffer.alloc(145);
      data.set(Buffer.from("DOPRV001")); data.fill(7, 8, 40);
      data.set(oracleState.base_side.asset_mint.toBytes(), 40);
      data.set(oracleState.quote_side.asset_mint.toBytes(), 72);
      data.writeBigUInt64LE(1_000_000_000n, 104);
      data.writeBigInt64LE(clock.unixTimestamp, 120);
      data.writeBigUInt64LE(clock.slot, 128);
      data.writeBigUInt64LE(1n, 136); data[144] = 1;
      svm.setAccount(oracleFeed, {data, owner: oracleProvider, lamports: 10_000_000n, executable: false});
    }
    let oracleReads = 0;
    const { lookupTable } = planWithTable(
      { ...f, internal: settlement === "dusk" },
      svm,
    );
    const requests: string[] = [];
    let simulations = 0;
    let clockMismatches = 0;
    const previewNames = ["preview_flash_liquidation", "preview_emergency_liquidation"];
    const previews: string[] = [];
    let rejectedDustProbes = 0;
    let fullEmergencyPreviews = 0;
    const fullEmergencyBounds: { reward: bigint; output: bigint }[] = [];
    const healthPreviews: { debt: bigint; collateralValue: bigint; maintenance: number }[] = [];
    const discriminator = (name: string) => createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);
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
            if (p[0].includes(oracleFeed.toBase58())) oracleReads++;
            answer = {
              context,
              value: p[0].map((key: string) =>
                rpcAccount(svm.getAccount(new PublicKey(key))),
              ),
            };
            if (clockMismatch && clockMismatches === 0 && p[0].includes(f.position) && p[0].includes("SysvarC1ock11111111111111111111111111111111")) {
              const account = answer.value[p[0].indexOf("SysvarC1ock11111111111111111111111111111111")];
              const data = Buffer.from(account.data[0], "base64");
              data.writeBigUInt64LE(BigInt(context.slot) + 1n, 0);
              account.data[0] = data.toString("base64");
              clockMismatches++;
            }
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
            // Removed devnet markets leave short position accounts behind.
            // The runtime must scope their identity before a full-layout read.
            if (
              !p[1].dataSlice &&
              answer.some((a: any) => a.pubkey === f.position)
            ) {
              const old = Buffer.alloc(f.kind === "BorrowPosition" ? 266 : 220);
              Buffer.from(bs58.decode(discriminator)).copy(old);
              new PublicKey(f.buyer).toBuffer().copy(old, 8);
              Buffer.alloc(32, 239).copy(old, 40);
              answer.push({
                pubkey: new PublicKey(Buffer.alloc(32, 238)).toBase58(),
                account: {
                  owner: p[0],
                  lamports: 1000000,
                  executable: false,
                  data: [old.toString("base64"), "base64"],
                },
              });
            }
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
            const invoked = previewNames.filter((name) => tx.message.compiledInstructions.some((ix) => Buffer.from(ix.data).subarray(0, 8).equals(discriminator(name))));
            previews.push(...invoked);
            for (const ix of tx.message.compiledInstructions) {
              const data = Buffer.from(ix.data);
              if (data.subarray(0, 8).equals(discriminator("emergency_liquidation")) && data[17]) {
                fullEmergencyBounds.push({ reward: data.readBigUInt64LE(18), output: data.readBigUInt64LE(26) });
              }
            }
            const emergency = tx.message.compiledInstructions.find(ix => Buffer.from(ix.data).subarray(0, 8).equals(discriminator("preview_emergency_liquidation")));
            if (emergency) {
              const data = Buffer.from(emergency.data);
              if (data[17]) fullEmergencyPreviews++;
              if (rejectEmergencyDust && data.readBigUInt64LE(9) === 1n && !data[17]) {
                rejectedDustProbes++;
                answer = { context, value: {
                  err: { InstructionError: [2, { Custom: 6032 }] },
                  logs: ["Program log: AnchorError thrown in programs/dusk/src/transitions/amm/mod.rs:1806. Error Code: InsufficientOutputAmount. Error Number: 6032. Error Message: Insufficient output amount."],
                } };
                break;
              }
            }
            if (rejectOrdinary && invoked.includes("preview_flash_liquidation")) {
              answer = { context, value: { err: "injected unexpected ordinary failure", logs: [] } };
              break;
            }
            const outcome: any = svm.simulateTransaction(tx);
            const failed = typeof outcome.err === "function";
            const meta = outcome.meta();
            const returned = meta.returnData();
            if (!failed && tx.message.compiledInstructions.some(ix => Buffer.from(ix.data).subarray(0, 8).equals(discriminator("preview_liquidation_health")))) {
              const data = Buffer.from(returned.data());
              healthPreviews.push({ collateralValue: data.readBigUInt64LE(0), debt: data.readBigUInt64LE(8), maintenance: data.readUInt16LE(16) });
            }
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
            if (driftEmergencyQuote && !failed && emergency && Buffer.from(emergency.data)[17]) {
              // The execution bank delivers one atom less than the preview.
              const quote = Buffer.from(answer.value.returnData.data[0], "base64");
              quote.writeBigUInt64LE(quote.readBigUInt64LE(41) + 1n, 41);
              answer.value.returnData.data[0] = quote.toString("base64");
            }
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
              KEEPER_LENDING_MODE: mode,
              KEEPER_SHADOW_PUBLIC_KEY: f.buyer,
              KEEPER_LOG_POSITIONS: "1",
              KEEPER_ALLOW_CAPTURED_DEVNET: devnetAllMarkets ? "1" : "0",
              KEEPER_MAX_SENDS_PER_PASS: maxSends,
              KEEPER_DEVNET_MARKET: new PublicKey(Buffer.alloc(32, 240)).toBase58(),
              PORT: "0",
              SOLANA_RPC_HTTP_URL: endpoint,
              KEEPER_LIQUIDATION_LOOKUP_TABLE: lookupTable,
              KEEPER_FLASH_SETTLEMENT: settlement,
              KEEPER_FLASH_MARKET: context.named.market,
              KEEPER_FLASH_MAX_REPAY: maxRepay,
              KEEPER_FLASH_MAX_PAYMENT_DEBIT: "18446744073709551615",
              KEEPER_FLASH_MIN_COLLATERAL_CREDIT:
                ["emergency", "unfilled", "error"].includes(expected) ? "18446744073709551615" : "1",
              KEEPER_LIQUIDATION_MIN_PROFIT: minProfit,
              KEEPER_EMERGENCY_SLIPPAGE_BPS: emergencySlippage,
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
      const diagnostic = ["monitor", "healthy", "debt_free"].includes(expected);
      const expectedOutput = expected === "monitor" ? /MONITOR ONLY: transaction budget used for this pass/
        : expected === "healthy" ? /state=HEALTHY/
        : expected === "debt_free" ? /state=DEBT_FREE health_factor=n\/a/
        : expected === "unfilled" ? /eligible but unfilled/
        : expected === "error" ? /injected unexpected ordinary failure/ : new RegExp(`shadow ${expected}`);
      const first = await once();
      assert.equal(first.code, 0, first.output);
      assert.match(first.output, expectedOutput);
      if (devnetAllMarkets) {
        assert.match(first.output, /execution_scope=all_supported_markets/);
        assert.ok(!first.output.includes("outside configured execution market"));
      }
      assert.ok(first.output.includes(`position_health kind=${f.kind} position=${f.position}`), first.output);
      assert.ok(first.output.includes(`market=${context.named.market}`));
      assert.match(first.output, /debt_mint=.+ collateral_mint=.+ snapshot_slot=\d+/);
      if (expected !== "debt_free") {
        assert.ok(healthPreviews.length > 0);
        assert.ok(first.output.includes(`debt_raw=${healthPreviews[0].debt}`), "log must use indexed debt returned by the program");
        assert.match(first.output, /liquidation_ltv_pct=\d+\.\d+ maintenance_pct=\d+\.\d+/);
      }
      const advanced = svm.getClock();
      advanced.slot += 1n;
      advanced.unixTimestamp += 1n;
      svm.setClock(advanced);
      const second = await once();
      assert.equal(second.code, 0, second.output);
      assert.match(second.output, expectedOutput);
      if (diagnostic) {
        assert.deepEqual(previews, [], "position diagnostics must not plan a liquidation");
        assert.equal(simulations, expected === "debt_free" ? 0 : 2);
      } else assert.ok(simulations >= 4);
      if (clockMismatch) assert.equal(clockMismatches, 1);
      if (rejectEmergencyDust) {
        assert.equal(rejectedDustProbes, 2, "each pass must reach the dust-size rejection");
        assert.equal(fullEmergencyPreviews, 2, "dust rejection must not abort the independent full-close preview");
        assert.match(first.output, /shadow emergency_full/);
        assert.ok(!first.output.includes("unexpected preview rejection"));
      }
      if (driftEmergencyQuote) {
        assert.ok(fullEmergencyBounds.length >= 2);
        for (const bounds of fullEmergencyBounds) {
          assert.ok(bounds.reward >= BigInt(minProfit), "minimum profit must be enforced by the transaction");
          assert.equal(bounds.output, emergencySlippage === "0" ? 9513n : emergencySlippage === "10000" ? 0n : 9504n);
          if (emergencySlippage === "10000") assert.equal(bounds.reward, BigInt(minProfit));
        }
      }
      if (oracle) assert.ok(oracleReads >= 2, "each fresh evaluation must fetch the active feed into its snapshot");
      if (mode === "emergency") assert.ok(!previews.includes("preview_flash_liquidation"));
      if (expected === "flash" || mode === "ordinary" || rejectOrdinary) assert.ok(!previews.includes("preview_emergency_liquidation"));
      if (expected === "emergency") {
        assert.ok(previews.includes("preview_emergency_liquidation"));
        if (mode === "both") {
          assert.equal(previews[0], "preview_flash_liquidation");
        }
      }
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
