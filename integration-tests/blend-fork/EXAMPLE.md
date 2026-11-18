# What happens when an epoch matures in a tight Blend pool?

This is a regression test for the liquidity risk described in #37. [Kanatoko](https://crates.io/crates/kanatoko) runs the current epoch manager and adapter against Blend testnet ledger 5648731 (Protocol 30). Pool utilisation starts at 94.7%. The test advances one year locally with no intervening pool activity.

| At this point | What the contracts do |
| --- | --- |
| Every 90 days | `refresh()` renews the adapter’s pool position. |
| At maturity | Accrued liabilities exceed supply. Blend rejects settlement with error 1207 and the epoch stays open. |
| After a lender tops up the pool | The same epoch settles. The senior depositor claims 1,050 XLM and the junior depositor claims the remainder of the redeemed value. |

The shortfall is specific to this frozen pool and these time advances. The test funds accounts locally and mocks authorization. It checks TTL renewal before expiry.

[Read the Rust assertions](tests/long_epoch.rs#L43).

## Reproduce locally

Use Rust 1.98.0 with the `wasm32v1-none` target and Stellar CLI (verified with 27.0.0). From the repo root, build the contracts, fetch the locked test dependencies once, then run the test offline.

```sh
stellar contract build --locked --package strata-epoch-manager --out-dir integration-tests/blend-fork/wasm
stellar contract build --locked --package strata-blend-adapter --out-dir integration-tests/blend-fork/wasm
cargo +1.98.0 fetch --locked --manifest-path integration-tests/blend-fork/Cargo.toml
cargo +1.98.0 test --locked --offline --manifest-path integration-tests/blend-fork/Cargo.toml
```

[Fixture attribution](NOTICE).
