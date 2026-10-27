# Devnet preview follow-up

Dusk source `77877d576018fd2bd8191d13f05534b0484e2282` fixes drained-market read previews and was upgraded at finalized slot 515502278. The vendored Dusk executable matches the tested payload SHA-256 `21e3f3c4dd3e0e5b4fad85f69e0bae702b1209ab4fe1820f6bc86e81408d07ed`. The deployed allocation adds 8,824 zero padding bytes. Its full SHA-256 is `d7eb11e277e4b215e0215fb926b353e145aac8452adc61adcbbbb432cffd71b9`.

The leverage delegate executable and SDK archive are unchanged. The delegate was not upgraded; its previously captured executable is retained rather than substituting a later local build. IDLs and event revision are unchanged. Liquidation and hLP fixtures were recaptured from the current Dusk binary.

The lock remains captured. All Railway keeper services remain shadow-only with signing disabled. These compatibility updates do not grant live execution authority.
