<p align="center">
  <img src="assets/logo.png" alt="Strata" width="128">
</p>

<p align="center">
  <a href="https://github.com/strata-protocol/strata-contracts/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/strata-protocol/strata-contracts/actions/workflows/ci.yml/badge.svg"></a>
  <a href="./LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/github/license/strata-protocol/strata-contracts"></a>
  <a href="https://developers.stellar.org/docs/build/smart-contracts/overview"><img alt="Stellar" src="https://img.shields.io/badge/Stellar-Soroban-7D00FF?logo=stellar&logoColor=white"></a>
</p>

# Strata

A senior/junior tranche wrapper for fixed-term epochs on Stellar Soroban.

> ## Disclaimer
>
> **Strata is unaudited and testnet-only.** Nothing in this repository has been
> reviewed by a third party. Do not deploy it to mainnet. Do not put real funds
> into it. The arithmetic is property-tested and the documented limits are
> derived rather than guessed, but that is not an audit and it is not a
> security review. See [`docs/risks.md`](docs/risks.md) for what is known to be
> unresolved, including four bugs that shipped and were caught by tests.

---

## What this is

A vault wrapper that splits the result of one fixed-term epoch between two
tranches:

- **Senior** — principal plus a fixed target rate, capped at that target, paid
  first.
- **Junior** — everything above the senior target, absorbing losses first.

The problem it addresses is that a fixed-rate promise and a variable underlying
pull in opposite directions. If the underlying earns less than the target, someone
has to take the shortfall; most structures let that land on the senior holder by
accident. Strata makes the junior tranche structurally senior instead: deposits are
gated so the junior buffer is always at least as large as the senior principal,
and settlement pays the senior first out of whatever the vault actually returned.

The interesting part is what it does *not* contain. There are no fees, no
re-entrancy surface of its own, no transferable positions, and no partial
withdrawal. All of the money-handling arithmetic lives in one small pure-Rust
crate with no `soroban-sdk` dependency at all, which is what makes it cheap to
property-test and impossible to disagree with the contract.

**It is not a yield product.** The vault it wraps is a mock whose yield an admin
sets by hand. There is no real yield source integrated.

## Architecture

```text
                    ┌──────────────────┐
                    │    waterfall     │  pure i128 settlement math
                    │  no dependencies │  no Env, no storage, no SDK
                    └────────┬─────────┘
                             │
                    ┌────────┴─────────┐        ┌───────────────────┐
                    │  epoch-manager   │───────▶│  vault-interface  │
                    │                  │        │  the ERC-4626     │
                    │  owns the money  │        │  subset Strata    │
                    │  and the state   │        │  needs            │
                    │  machine         │        └─────────┬─────────┘
                    └────────┬─────────┘                  │
                             │                   ┌────────┴─────────┐
                             └───────────────────▶│   mock-vault     │
                                                 │  admin-set signed│
                                                 │  yield (testnet) │
                                                 └──────────────────┘
```

The contract owns the money and the state machine. It owns **none** of the
arithmetic: every payout figure comes from the `waterfall` crate. That is why
there is exactly one implementation of the split to audit — and why
[`contracts/waterfall/src/lib.rs`](contracts/waterfall/src/lib.rs) is the first
file to read.

Details in [`docs/architecture.md`](docs/architecture.md). The settlement math and
its five invariants are specified in
[`docs/waterfall-spec.md`](docs/waterfall-spec.md), which is the source of truth —
code that disagrees with it is a bug, and changing it needs a `spec` issue.

### The waterfall in one table

`S = J = 1_000_000`, senior target 5%/yr, one-year term, so the senior's target is
`1_050_000`. If the vault returns `V`:

| `V` | senior_payout | junior_payout | what happened |
| --- | --- | --- | --- |
| 2_200_000 | 1_050_000 | 1_150_000 | healthy; senior capped, junior takes the rest |
| 1_100_000 | 1_050_000 | 50_000 | a 900_000 loss, fully absorbed by the buffer |
| 1_049_999 | 1_049_999 | 0 | one unit past the cushion; senior starts paying |
| 900_000 | 900_000 | 0 | junior wiped, senior covers the shortfall |
| 0 | 0 | 0 | total wipeout |

`senior_payout + junior_payout == V` always. The senior never beats its target.
The junior is paid nothing unless the senior was made whole first.

The cushion is `J - I`, where `I` is the senior's *own* target interest — not `J`.
A buffer exactly equal to `I` leaves zero cushion. This is the most misreadable
property of the design; see [the spec](docs/waterfall-spec.md#why-these-bounds)
and [risks.md](docs/risks.md). A senior depositor can lose their entire principal
in a wipeout. "Senior" means paid first, not safe.

## Quickstart

Requires a Rust toolchain (pinned in `rust-toolchain.toml`) and, for the Wasm
build, the Stellar CLI. A prebuilt binary from the official
[stellar-cli releases page](https://github.com/stellar/stellar-cli/releases) is
much faster to install than `cargo install stellar-cli`.

```bash
git clone https://github.com/strata-protocol/strata-contracts
cd strata-contracts

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --locked

# Stronger property testing, which is where a settlement bug would surface.
PROPTEST_CASES=20000 cargo test --release -p strata-tests
```

`cargo test` needs no network access and no credentials. All five of the checks
above were run against this tree.

### Building the contracts

```bash
stellar contract build
# -> target/wasm32v1-none/release/strata_epoch_manager.wasm   (25964 bytes)
# -> target/wasm32v1-none/release/strata_mock_vault.wasm     ( 9638 bytes)
```

`cargo build` does **not** produce a deployable contract. The Soroban runtime only
accepts `wasm32v1-none` built by `stellar contract build`, which also runs wasm-opt
— which is why its artefacts are smaller than a plain `cargo build`'s. CI builds
the real thing and fails if either named artefact is missing.

### Deploying and running on testnet

Both contracts are live. Full guide in [`docs/deployment.md`](docs/deployment.md);
machine-readable record in [`deployments/testnet.json`](deployments/testnet.json).

```bash
./scripts/deploy-testnet.sh --self-test                      # no network needed
./scripts/deploy-testnet.sh strata-deployer testnet          # fresh deploy

./scripts/run-epoch-demo.sh good                             # ~6 min, mostly waiting
./scripts/run-epoch-demo.sh loss                             # ~6 min, mostly waiting
```

Every script refuses any network but testnet, and CI runs that refusal as a test.

## Live demo

A read-only dashboard shows these contracts' live testnet state: the current
epoch, an account's tranche position, and projected payouts.

**https://strata-protocol.github.io/strata-app/**

Testnet only, unaudited. If testnet has been reset, the dashboard will report
that the contracts were not found.

## Testnet deployment

Deployed 2026-10-05 with Stellar CLI 28.1.0 and soroban-sdk 27.0.6, against a
protocol 29 testnet. The on-chain Wasm hash of each contract was checked against
the sha256 of the locally built artefact and matched exactly.

| Contract | Address | Explorer |
| --- | --- | --- |
| `epoch-manager` | `CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU` | [stellar.expert](https://stellar.expert/explorer/testnet/contract/CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU) |
| `mock-vault` | `CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6` | [stellar.expert](https://stellar.expert/explorer/testnet/contract/CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6) |
| Underlying (native XLM SAC) | `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC` | [stellar.expert](https://stellar.expert/explorer/testnet/contract/CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC) |

Two complete epochs were settled on that deployment, with every figure checked
against [`docs/waterfall-spec.md`](docs/waterfall-spec.md) recomputed independently
by the script rather than read back from the contract:

| Scenario | `V` redeemed | `senior_payout` | `junior_payout` | Demonstrates |
| --- | --- | --- | --- | --- |
| `good` | 2 000 204 528 | 1 000 009 512 | 1 000 195 016 | the cap binding: senior took its 9 512 target, junior the other 195 016 |
| `loss` | 1 999 836 692 | 1 000 009 512 | 999 827 180 | senior made whole, junior absorbed the whole 172 820 loss |

In both, the two payouts summed to `V` to the stroop. Transaction hashes are in
[`docs/deployment.md`](docs/deployment.md).

Testnet is reset periodically by SDF, so these addresses are only meaningful
until the next reset.

## Status

Marked **Done** only where the work was actually run and checked.

| Component | Status | Notes |
| --- | --- | --- |
| `docs/waterfall-spec.md` | **Done** | Source of truth. Frozen without a `spec` issue. |
| `contracts/waterfall` | **Done** | Pure settlement math. 19 tests. |
| Five spec invariants | **Done** | 16 property tests in `tests/src/waterfall_props.rs`. |
| `contracts/vault-interface` | **Done** | The ERC-4626 subset, plus `asset` and `balance_of`. No tests of its own. |
| `contracts/mock-vault` | **Done** | Test double, admin-set signed yield. 21 tests. |
| `contracts/epoch-manager` | **Done** | Epoch lifecycle, deposits, settlement, claims. 18 tests. |
| Cross-contract tests | **Done** | 30 tests over manager + vault + a real token. |
| CI | **Done** | fmt, clippy, test, wasm build, spec consistency, audit. |
| Testnet deployment | **Done** | Two epochs settled and checked against the spec. |
| `docs/architecture.md`, `docs/risks.md` | **Done** | Data flow and authorisation; working risk register. |
| TTL audit for per-user positions (R9) | **Planned** | Open. Issue #10. |
| External audit | **Planned** | Not requested. Issue #6. |
| `strata-app` SDK + dashboard | **In progress** | Separate repo, not created yet. |
| Mainnet support | **Out of scope** | Deliberately. This project will not do it. |

Nothing in this table is placeholder code presented as finished. "Planned" and
"not started" are stated as they are.

### Tests

**104 tests, all passing.** Counted by `cargo test --all --locked`, one `#[test]`
per case:

| Where | Tests | What it proves |
| --- | --- | --- |
| `contracts/waterfall/src/tests.rs` | 19 | the spec's worked examples and boundaries, by name |
| `contracts/mock-vault/src/test.rs` | 21 | the yield model, so integration results mean something |
| `contracts/epoch-manager/src/test.rs` | 18 | lifecycle transitions, bounds, who may do what |
| `tests/src/waterfall_props.rs` | 16 | all five spec invariants as properties |
| `tests/src/integration.rs` | 30 | a real manager, vault and SEP-41 token, end to end |
| `contracts/vault-interface` | 0 | a trait declaration, nothing to test |

Every settlement assertion in the integration tests is checked against
`strata_waterfall::settle` computed independently, so the contract's numbers are
never taken on trust. Four bugs that shipped and were caught by these tests are
recorded as **[realised]** in [`docs/risks.md`](docs/risks.md), including a
pro-rata denominator that shrank on every claim and would have overpaid the senior
tranche.

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) first, then pick something from the
[open issues](https://github.com/strata-protocol/strata-contracts/issues).

- **Comment on an issue before you start**, so only one person works on it at a
  time.
- **The waterfall spec is frozen.** Disagree with the math? Open a `spec` issue.
  Do not change the numbers in a pull request.
- **Every invariant gets a test.** New arithmetic needs new property tests.
- **No placeholder code presented as finished.** Unfinished work is an open issue.
- **Testnet only.**

Complexity and Points are assigned in the Drips Wave dashboard when an issue is
added to a Program, not by a label in this repository. `CONTRIBUTING.md` explains
what the documentation actually says.

Security reports go through [`SECURITY.md`](SECURITY.md), not a public issue.

## Maintainers

| Role | Contact |
| --- | --- |
| Maintainer | [`sulaimonifeoluwa4-blip`](https://github.com/sulaimonifeoluwa4-blip) |
| Security contact | `sulaimonifeoluwa4@gmail.com` — see [`SECURITY.md`](SECURITY.md) |
| Community channel | https://t.me/+N9ZmMAjKnCpjZWI8 |

## Documentation

| Document | What is in it |
| --- | --- |
| [`docs/waterfall-spec.md`](docs/waterfall-spec.md) | The settlement math and its five invariants. Source of truth. |
| [`docs/architecture.md`](docs/architecture.md) | Layout, data flow, storage, authorisation. |
| [`docs/risks.md`](docs/risks.md) | Risk register, including four realised bugs. |
| [`docs/deployment.md`](docs/deployment.md) | Testnet deployment, reproduction steps, known issues. |
| [`docs/demo-runbook.md`](docs/demo-runbook.md) | Script for recording a walkthrough demo. |
| [`docs/maintaining.md`](docs/maintaining.md) | Branch protection and release process. |
| [`docs/release-notes-v0.1.0.md`](docs/release-notes-v0.1.0.md) | Draft release notes. Not tagged, not released. |

## Related repositories

- [`strata-app`](https://github.com/strata-protocol/strata-app) — TypeScript SDK
  generated from these contract specs, pinned to a contract version, plus the
  dashboard. In progress; the repository does not exist yet.

## Safety notes

This repository contains smart contract code. Review deployment steps carefully
before using any live network or production asset. Keep private keys, RPC
credentials and wallet secrets out of commits, issue comments and logs. Identities
for the testnet scripts live in the Stellar CLI's own store, outside the repository.

## License

[Apache-2.0](LICENSE), matching `soroban-sdk` and Stellar's own tooling.

## Contributors

Thanks to everyone who has contributed.

[![Contributors](https://contrib.rocks/image?repo=strata-protocol/strata-contracts)](https://github.com/strata-protocol/strata-contracts/graphs/contributors)
