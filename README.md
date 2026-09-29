# Strata

A tranche wrapper on Stellar Soroban. Strata wraps any ERC-4626-style vault and
splits the result of one fixed-term epoch into two tranches:

- **Senior** — principal plus a fixed target rate, capped at that target, paid
  first.
- **Junior** — everything above the senior target, absorbing losses first.

The senior's claim is protected by the junior buffer, and deposits are gated so
the buffer is always real.

> ## Disclaimer
>
> **Strata is unaudited and testnet-only.** Nothing in this repository has been
> reviewed by a third party. Do not deploy it to mainnet. Do not put real funds
> into it. The arithmetic is property-tested and the documented limits are
> derived rather than guessed, but that is not an audit and it is not a
> security review. See [`docs/risks.md`](docs/risks.md) for what is known to be
> unresolved.

---

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
arithmetic: every payout figure comes from the `waterfall` crate, which is plain
Rust with no `soroban-sdk` dependency at all. That is what makes the property
tests fast, and it means there is exactly one implementation of the split.

Details in [`docs/architecture.md`](docs/architecture.md). The settlement math
and its five invariants are specified in
[`docs/waterfall-spec.md`](docs/waterfall-spec.md), which is the source of
truth — code that disagrees with it is a bug.

## The waterfall in one example

`S = J = 1_000_000`, senior target 5%/yr, one-year term, so the senior's target
is `1_050_000`. If the vault returns `V`:

| `V` | senior_payout | junior_payout | what happened |
| --- | --- | --- | --- |
| 2_200_000 | 1_050_000 | 1_150_000 | healthy; senior capped, junior takes the rest |
| 1_100_000 | 1_050_000 | 50_000 | a 900_000 loss, fully absorbed by the buffer |
| 1_049_999 | 1_049_999 | 0 | one unit past the cushion; senior starts paying |
| 900_000 | 900_000 | 0 | junior wiped, senior covers the shortfall |
| 0 | 0 | 0 | total wipeout |

`senior_payout + junior_payout == V` always. The senior never beats its
target. The junior is paid nothing unless the senior was made whole first.

The cushion is `J - I`, where `I` is the senior's own target interest — not `J`.
A buffer exactly equal to `I` gives zero cushion. This is the single most
misreadable property of the design and it is called out in
[the spec](docs/waterfall-spec.md#why-these-bounds) and in
[risks.md](docs/risks.md).

## Status

| Component | Status | Notes |
| --- | --- | --- |
| `docs/waterfall-spec.md` | **Done** | Source of truth. Frozen without a `spec` issue. |
| `contracts/waterfall` | **Done** | Pure settlement math. 19 unit tests. |
| Five spec invariants | **Done** | Property tests, `tests/src/waterfall_props.rs`. |
| `contracts/vault-interface` | **Done** | The ERC-4626 subset, plus `asset` and `balance_of`. |
| `contracts/mock-vault` | **Done** | Test double, admin-set signed yield. 21 tests. |
| `contracts/epoch-manager` | **Done** | Epoch lifecycle, deposits, settlement, claims. |
| Cross-contract tests | **Done** | 46 tests over manager + vault + a real token. |
| `docs/architecture.md` | **Done** | Layout, data flow, storage, authorisation. |
| `docs/risks.md` | **Done** | Working risk register, including realised bugs. |
| CI | **Done** | fmt, clippy, test, wasm build, spec consistency, audit. |
| `scripts/deploy-testnet.sh` | **Unverified** | Network guard tested, wasm discovery tested. Never run against a live network. |
| Testnet deployment | **Not started** | Blocked on funded testnet credentials. |
| `strata-app` SDK + dashboard | **Not started** | Separate repo. Needs a pinned contract version. |
| External audit | **Not started** | Not requested. |
| Mainnet support | **Out of scope** | Deliberately. This project will not do it. |

"Unverified" and "not started" are stated as they are. Nothing in this table is
placeholder code presented as finished.

## Quickstart

Requirements: a Rust toolchain (pinned in `rust-toolchain.toml`) and the Stellar
CLI for the Wasm build.

```bash
git clone https://github.com/strata-finance/strata-contracts
cd strata-contracts

cargo test                                     # the whole suite
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check

# Stronger property testing, which is where a settlement bug would surface.
PROPTEST_CASES=20000 cargo test --release -p strata-tests
```

`cargo test` runs the property tests, the mock vault's unit tests, the epoch
manager's state-machine tests and the cross-contract integration tests. No
network access and no credentials are needed for any of it.

### Building the contracts

```bash
cargo install stellar-cli --locked
stellar contract build
# -> target/wasm32v1-none/release/strata_epoch_manager.wasm
# -> target/wasm32v1-none/release/strata_mock_vault.wasm
```

`cargo build` does **not** produce a deployable contract. The Soroban runtime
only accepts `wasm32v1-none` built by `stellar contract build`.

To check the contracts compile for the Wasm target without installing the CLI:

```bash
cargo build --target wasm32v1-none --release \
  -p strata-epoch-manager -p strata-mock-vault
```

That verifies the `no_std` code, which is the part most likely to break, but
the resulting artefacts are **not** deployable — `stellar contract build`
applies further build settings the runtime requires. This has been run and both
contracts compile clean; `stellar contract build` itself is exercised only in
CI.

### Deploying to testnet

```bash
./scripts/deploy-testnet.sh <path-to-identity-file> testnet
```

The script refuses any network outside a hardcoded allowlist, and CI runs that
refusal as a test. It has otherwise never been run — see the status table.

### Reading the code

If you only read one thing, read
[`contracts/waterfall/src/lib.rs`](contracts/waterfall/src/lib.rs). It is
small, has no dependencies, and is the only place in the repository where money
is divided.

## Testing approach

| Layer | What it proves |
| --- | --- |
| Spec arithmetic (`contracts/waterfall/src/tests.rs`) | The worked examples and boundaries, by name. |
| Invariants (`tests/src/waterfall_props.rs`) | All five spec invariants as properties over generated inputs, including at the `senior_due` boundary. |
| Yield model (`contracts/mock-vault/src/test.rs`) | The test double is trustworthy, so integration results mean something. |
| State machine (`contracts/epoch-manager/src/test.rs`) | Lifecycle transitions, bounds, and who may do what. |
| Whole system (`tests/src/integration.rs`) | A real manager, vault and SEP-41 token, end to end. |

Every settlement assertion in the integration tests is checked against
`strata_waterfall::settle` computed independently, so the contract's numbers
are never taken on trust. The suite is currently 104 tests, all passing.

Four bugs that shipped and were caught by these tests are recorded as
**[realised]** in [`docs/risks.md`](docs/risks.md), including a pro-rata
denominator that shrank on every claim and would have overpaid the senior
tranche, and an `MAX_ASSETS` literal that was 1000x its documented value and
silently voided the whole overflow argument.

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) first. The parts that matter most:

- **The waterfall spec is frozen** without discussion at the table. Disagree
  with the math? Open a `spec` issue. Do not change the numbers in a PR.
- **Every invariant gets a test.** New arithmetic needs new property tests.
- **No placeholder code presented as finished.** Unfinished work is an open
  issue.
- **Testnet only.**

Issues carry a `drips:N` label, and `N` is the Wave point value of the work:
`drips:1` small, `drips:3` medium, `drips:5` large, `drips:8` complex.

Security reports go through [`SECURITY.md`](SECURITY.md), not a public issue.

## Related repositories

- [`strata-app`](https://github.com/strata-finance/strata-app) — TypeScript SDK
  generated from these contract specs, pinned to a contract version, plus the
  dashboard. Not started.

## License

[Apache-2.0](LICENSE), matching `soroban-sdk` and Stellar's own tooling.
