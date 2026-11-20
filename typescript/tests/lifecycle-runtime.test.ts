import assert from "node:assert/strict";
import test from "node:test";
import { PublicKey } from "@solana/web3.js";
import { result } from "./helpers/liquidation-bank.ts";
import {
  fixture,
  run,
  installLookupTable,
  pda,
  u64,
  program,
  key,
  tableOwner,
} from "./helpers/lifecycle-bank.ts";

for (const change of ["manager", "recipient", "handoff"] as const) {
  for (const withTable of [false, true]) {
    test(`native lifecycle executes ${change} ${withTable ? "with lookup table" : "without lookup table"}`, async () => {
      const f = fixture(change);
      f.advance(BigInt(f.notice.execute_after_slot.toString()));
      const table = withTable ? installLookupTable(f) : undefined;
      const signer = change === "handoff" ? f.next : f.payer;
      const first = await run(f, signer, table);
      assert.match(
        first.output,
        /simulation passed; no new signature produced/,
      );
      assert.equal(first.simulations.length, 1);
      const { transaction, error } = first.simulations[0];
      assert.equal(error, null);
      assert.equal(transaction.message.header.numRequiredSignatures, 1);
      assert.ok(transaction.message.staticAccountKeys[0].equals(signer));
      assert.equal(transaction.version, withTable ? 0 : "legacy");
      if (table) {
        assert.ok(
          first.requests.some(
            (r) =>
              r.method === "getMultipleAccounts" &&
              r.params[0].includes(table.toBase58()),
          ),
        );
        assert.ok(
          transaction.message.addressTableLookups.some((lookup) =>
            lookup.accountKey.equals(table),
          ),
        );
      }
      // Commit the worker's exact simulated plan locally and verify the resulting state.
      assert.equal(result(f.svm, transaction), null);
      const after = f.state("Market", f.market);
      assert.ok(after.operator.pending.equals(PublicKey.default));
      assert.ok(f.state("OperatorProposal", f.proposal).executed);
      if (change === "manager") {
        assert.ok(after.operator.manager.equals(f.manager));
      } else {
        const archive = f.state(
          "OperatorRevenueAccount",
          pda(Buffer.from("operator_revenue"), f.market.toBuffer(), u64(0n)),
        );
        assert.ok(archive.sealed);
        assert.ok(archive.recipient.equals(f.payer));
        assert.ok(
          after.operator.revenue_recipient.equals(
            change === "handoff" ? f.next : f.recipient,
          ),
        );
        if (change === "handoff")
          assert.ok(after.operator.authority.equals(f.next));
      }
      const again = await run(f, signer, table);
      assert.equal(
        again.simulations.length,
        0,
        "completed bundle is not executed twice",
      );
    });
  }
}

for (const condition of [
  "not ready",
  "expired",
  "stale epoch",
  "wrong successor",
] as const) {
  test(`native lifecycle rejects ${condition}`, async () => {
    const f = fixture("handoff");
    if (condition === "expired")
      f.advance(BigInt(f.notice.deadline_slot.toString()) + 1n);
    else if (condition !== "not ready")
      f.advance(BigInt(f.notice.execute_after_slot.toString()));
    if (condition === "stale epoch") {
      const market = f.state("Market", f.market);
      market.operator.epoch = market.operator.epoch.addn(1);
      f.updateMarket(market);
    }
    const before = Buffer.from(f.svm.getAccount(f.market)!.data);
    const checked = await run(
      f,
      condition === "wrong successor" ? f.payer : f.next,
    );
    assert.doesNotMatch(checked.output, /simulation passed/);
    const reason = {
      "not ready": /OperatorNoticeNotReady/,
      expired: /OperatorProposalExpired/,
      "stale epoch": /operator epoch changed/,
      "wrong successor": /successor acceptance requires its own signer/,
    }[condition];
    assert.match(checked.output, reason);
    assert.ok(checked.simulations.every((s) => s.error !== null));
    assert.deepEqual(Buffer.from(f.svm.getAccount(f.market)!.data), before);
  });
}

for (const condition of [
  "missing",
  "wrong owner",
  "deactivated",
  "not warmed up",
] as const) {
  test(`native lifecycle refuses ${condition} lookup table`, async () => {
    const f = fixture("handoff");
    f.advance(BigInt(f.notice.execute_after_slot.toString()));
    const table =
      condition === "missing"
        ? key("absent-lifecycle-table")
        : installLookupTable(f);
    if (condition !== "missing") {
      const data = Buffer.from(f.svm.getAccount(table)!.data);
      if (condition === "deactivated") data.writeBigUInt64LE(0n, 4);
      if (condition === "not warmed up")
        data.writeBigUInt64LE(f.svm.getClock().slot, 12);
      f.put(table, condition === "wrong owner" ? program : tableOwner, data);
    }
    const checked = await run(f, f.next, table);
    assert.equal(checked.simulations.length, 0);
    assert.match(
      checked.output,
      condition === "missing"
        ? /missing data account/
        : condition === "wrong owner"
          ? /invalid liquidation lookup table/
          : /lookup table deactivated or not warmed up/,
    );
    assert.doesNotMatch(checked.output, /simulation passed/);
  });
}
