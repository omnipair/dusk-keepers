# Auction inventory and prerequisites

The auction worker buys protocol revenue with the auction curve's accepted mint.
Native SOL funds transaction fees and rent; it is not a WSOL token balance.
It does not automatically sell purchased assets on Jupiter or replenish WSOL.
A configured maximum payment is a spending ceiling, not a profitability check.

Before activation:

1. Read the verified market, each lane/source liability, accepted mint and auction
   reserves directly from RPC. Governance must configure any curve with accrued
   inventory before it can settle. Funding cannot repair an unconfigured curve.
2. Derive the bidder's payment and receive ATAs with each mint's owning token
   program. Treasury and staking payment ATAs must also exist, even for a zero
   distribution share. A PDA treasury requires off-curve ATA derivation.
3. Verify current account sizes/rent, native reserve and fees independently from
   accepted-token working capital. Token-2022 receive accounts may require
   additional extensions. Quote account creation by simulation before paying rent.
4. Quote the actual bounded lot. `KEEPER_AUCTION_MAX_SOLD` caps the freshly read
   liability; small accruals do not need to reach that maximum. Payment measurement
   remains unsigned; final submissions retain payment/slippage ceilings, direct
   revalidation, simulation, the durable journal and distributed lease.
5. Review WSOL funding and any governance curve changes separately. Working
   capital must cover the selected bounded payments and replenishment interval;
   it cannot be inferred from the native fee balance. Do not count purchased token
   inventory as immediately reusable WSOL.

Empty liabilities are ordinary idle lanes. An accrued liability with no configured
payment mint or nonzero virtual reserves is an explicit preparation error. The
worker does not create token accounts, wrap SOL, alter governance settings or
weaken the frozen protocol/release gates to resolve these prerequisites.

For the current OMFG/USDv rollout, verify all eight fee/buyback × swap/interest ×
base/quote curves. A configured lane and funding elsewhere do not imply that
another lane is executable. Use the live dashboard/logs and current chain state;
this document intentionally does not embed an expiring funding recommendation.
