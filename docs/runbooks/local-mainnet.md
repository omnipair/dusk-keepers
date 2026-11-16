# Local mainnet keeper

The current mainnet artifact lock is frozen under the owner's scoped
[release-test waiver](../evidence/mainnet-release-waiver.json). The full release
matrix and production canary were not completed. Runtime deployment checks,
simulations, fee/reserve checks and the durable journal remain required.

## Setup

From this checkout, with Python 3, Rust and PostgreSQL 17 installed:

```sh
cargo build --locked --bin dusk-keeper
python3 scripts/local-mainnet.py setup
```

The launcher creates private files under `~/.local/share/dusk-keepers/mainnet`.
It never replaces an existing key, RPC URL, policy or settings file. It uses a
separate database from devnet. Setup starts no workers and makes no RPC calls.

1. Put your mainnet RPC URL in `rpc-url`.
2. Put the leverage wallet's private key in `wallets/leverage.key`, using a local
   editor. Accepts a base58 wallet export or the Solana CLI 64-byte JSON array;
   seed phrases and 32-byte seeds are not supported. Keep permissions at `0600`.
   Never paste the key in chat, a command argument, Git or a log.
3. Review `risk-policy.json`. Defaults retain 0.05 SOL and allow at most 0.0002
   SOL in fees per transaction / 0.02 SOL per UTC day. Inventory token payments
   have no per-token caps. Fund the wallet with SOL above its reserve, plus the
   debt tokens needed for inventory liquidations.

To check the signer identity, deployed programs and database without signing or
submitting any transaction:

```sh
python3 scripts/local-mainnet.py check leverage
```

This prints only the wallet's public address. It checks dependencies, not wallet
funding or whether an eligible transaction will execute. The worker checks those
conditions on each live attempt.

## Run

```sh
python3 scripts/local-mainnet.py terminal leverage
```

This starts **live mainnet execution**. Leverage handles leveraged-position
liquidations, take-profit, stop-loss and entry/limit orders. It scans all
supported markets, including `7xf68VEvMtLJ2Up5zVZdA161uH1yNkTFYgDFhNUSwkM7`;
the supplied market is not an execution restriction. The interval is five seconds,
with one submission per pass. Ordinary liquidations pay from inventory and retain
collateral. Zero-bounty orders are allowed. Emergency slippage defaults to 1%;
an explicit `KEEPER_EMERGENCY_SLIPPAGE_BPS` from 0 through 10000 overrides it.

Ctrl+C stops the workers, then the database if this session started it. The
journal and wallets remain on disk. Restart with the same directory to preserve
pending-transaction reconciliation and fee history. Logs are timestamped and
colored in a terminal; plain logs are saved under `logs/`.

To also run borrowing liquidation, lifecycle and monitoring, supply distinct
keys in `wallets/lending.key` and `wallets/lifecycle.key`, then run:

```sh
python3 scripts/local-mainnet.py terminal lending leverage lifecycle sentinel
```

Each signing service needs its own wallet. Duplicate public keys within a session
are rejected. Sentinel needs no key. hLP and auctions are excluded unless selected
explicitly. To enable them, add their key files and the required execution bounds
from [`env.example`](../../deploy/mainnet/env.example) to their `environment` object
in `settings.json`, then select `hlp-recovery` or `auction-arbitrage` on the command.
All selected profiles must pass preflight before any begins execution.

`settings.json` can point `keyPath` to an existing private key file instead of the
default. Optional `KEEPER_MAINNET_HOME` relocates the private state directory;
do not use a new directory to evade the existing journal or fee history.
`KEEPER_POSTGRES_BIN` overrides `/opt/homebrew/opt/postgresql@17/bin`. PostgreSQL
listens only on a private Unix socket (port identifier 15440); no TCP listener is
opened. Health endpoints use ports 18091–18096. Only one launcher session may use
the state directory at a time.
