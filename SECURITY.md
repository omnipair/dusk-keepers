# Security policy

Do not open a public issue for suspected fund-loss, signing, authorization, or
market-manipulation vulnerabilities. Use the private security-reporting channel
configured by the repository owner.

Never attach seed phrases, keypair files, RPC credentials, database URLs, or raw
production logs. Reports should include the affected protocol revision, keeper
profile, observed slot, sanitized candidate ID, and a minimal reproduction.

Live keeper deployments require a reviewed frozen protocol lock, a remote signer
restricted to the service wallet, an allowlisted market set, and an independent
transaction-value/rate policy at the signer boundary.

