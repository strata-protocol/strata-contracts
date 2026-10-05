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
| `scripts/deploy-testnet.sh` | **Done** | Run against testnet on 2026-10-05. Mainnet guard tested; deployment verified. |
| Testnet deployment | **Done** | [epoch-manager](https://stellar.expert/explorer/testnet/contract/CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU) and [mock-vault](https://stellar.expert/explorer/testnet/contract/CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6) live on testnet. See [`docs/deployment.md`](docs/deployment.md). |
| `scripts/run-epoch-demo.sh` | **Done** | A real good epoch and a real loss epoch, settled and checked against the spec on testnet. |
| `scripts/extend-ttl-testnet.sh` | **Done** | Extends the deployed instances and Wasm. Does **not** close R9. |
| TTL audit for per-user positions (R9) | **Not started** | Still open. See [`docs/risks.md`](docs/risks.md). |
| `strata-app` SDK + dashboard | **Not started** | Separate repo. Needs a pinned contract version. |
| External audit | **Not started** | Not requested. |
| Mainnet support | **Out of scope** | Deliberately. This project will not do it. |

"Unverified", "not started" and "out of scope" are stated as they are. Nothing
in this table is placeholder code presented as finished.

### What ran on testnet

Not a simulation, and not the test suite. Real transactions on Stellar testnet,
settled against the spec:

| Scenario | `V` redeemed | `senior_payout` | `junior_payout` | Result |
| --- | --- | --- | --- | --- |
| `good` | 2 000 204 528 | 1 000 009 512 | 1 000 195 016 | senior capped at its target, junior took the excess |
| `loss` | 1 999 836 692 | 1 000 009 512 | 999 827 180 | senior made whole, junior absorbed the loss |

In both, the two payouts summed to `V` exactly. Transaction hashes are in
[`docs/deployment.md`](docs/deployment.md).

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

Install the Stellar CLI once — a prebuilt binary from the official
[stellar-cli releases page](https://github.com/stellar/stellar-cli/releases) is
much faster than `cargo install`:

```bash
stellar contract build
# -> target/wasm32v1-none/release/strata_epoch_manager.wasm   (25964 bytes)
# -> target/wasm32v1-none/release/strata_mock_vault.wasm     ( 9638 bytes)
```

`cargo build` does **not** produce a deployable contract. The Soroban runtime
only accepts `wasm32v1-none` built by `stellar contract build`.

To check the contracts compile for the Wasm target without installing the CLI:

```bash
cargo build --target wasm32v1-none --release \
  -p strata-epoch-manager -p strata-mock-vault
```

That verifies the `no_std` code, which is the part most likely to break, but
the resulting artefacts are **not** deployable — `stellar contract build` runs
wasm-opt and applies further build settings the runtime requires. The artefacts
it produces are smaller than a plain `cargo build`'s for that reason. This was
run for both contracts and they compile clean; CI runs the real
`stellar contract build`.

### Deploying to testnet

It is deployed. Current contract IDs are in
[`deployments/testnet.json`](deployments/testnet.json), and
[`docs/deployment.md`](docs/deployment.md) has the full guide.

To deploy a fresh pair:

```bash
stellar keys generate --network testnet --fund strata-deployer
./scripts/deploy-testnet.sh strata-deployer testnet
```

The script refuses any network outside a hardcoded allowlist, and CI runs that
refusal as a test. It writes `deployments/testnet.json` with the contract IDs,
wasm hashes and CLI and SDK versions.

To watch a whole epoch run, and have every figure checked against
[`docs/waterfall-spec.md`](docs/waterfall-spec.md):

```bash
./scripts/run-epoch-demo.sh good   # ~6 minutes, mostly waiting
./scripts/run-epoch-demo.sh loss   # ~6 minutes, mostly waiting
```

[`docs/demo-runbook.md`](docs/demo-runbook.md) is the screen-recording script.

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
