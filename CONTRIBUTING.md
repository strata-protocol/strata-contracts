# Contributing to Strata

Thanks for your interest. Strata is an **unaudited, testnet-only** research
implementation of a tranche wrapper on Stellar Soroban. Read
[the disclaimer](README.md#disclaimer) before contributing.

## Ground rules

1. **Testnet only.** Nothing in this repository may be used to deploy to
   mainnet. Deployment tooling must refuse a mainnet network passphrase.
2. **The waterfall spec is frozen without discussion at the table.** If you
   believe the settlement math in [`docs/waterfall-spec.md`](docs/waterfall-spec.md)
   is wrong, open an issue and describe the failing case. Do not change the
   numbers, the branching condition, or the invariants in a pull request.
   Code that disagrees with the spec is a bug, and a spec change is a
   maintainer decision made in an issue.
3. **No placeholder code presented as finished.** Partially built features
   belong in an open issue, not in a merged branch.
4. **Every invariant gets a test.** The five invariants in the spec each have
   at least one property test. New math needs new property tests.

## Getting set up

```bash
git clone https://github.com/strata-protocol/strata-contracts
cd strata-contracts
rustup show                 # rust-toolchain.toml pins the toolchain + wasm target
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Install the Stellar CLI to build deployable Wasm. `cargo build` will **not**
produce a usable contract; the Soroban runtime only accepts `wasm32v1-none`
builds produced by `stellar contract build`, which also runs wasm-opt.

A prebuilt binary from the official
[stellar-cli releases page](https://github.com/stellar/stellar-cli/releases) is
much faster to install than `cargo install`:

```bash
stellar contract build      # -> target/wasm32v1-none/release/*.wasm
```

## Claiming an issue

1. Read the issue and the ground rules above.
2. **Comment on the issue before you start**, saying you are taking it. One
   contributor per issue at a time.
3. Ask to be assigned if you would like the assignment recorded. A maintainer
   can assign you directly on GitHub.
4. If the work turns out to be bigger than the issue describes, say so in the
   thread before you go further, rather than widening the pull request
   silently.

The backlog is the [open issues](https://github.com/strata-protocol/strata-contracts/issues).
Issues #4 to #11 are reserved for outside contributors and have deliberately not
been started.

## Complexity and Wave Points

Complexity is **not** set by a label in this repository. Per the
[Drips Wave documentation](https://docs.drips.network/wave/maintainers/participating-in-a-wave),
a maintainer assigns a complexity level in the Drips dashboard when adding an
issue to a Wave Program, and that level determines the Points:

| Complexity | Points | The documentation describes it as |
| --- | --- | --- |
| Trivial | 100 (base) | typos, small bug fixes, minor copy changes |
| Medium | 150 (base + 50 complexity bonus) | standard features, involved bug fixes |
| High | 200 (base + 100 complexity bonus) | complex features, refactors, new integrations |

The same page describes a "GitHub Label Workflow": once a repository is approved
for a Program, the Drips Wave bot comments on an issue and applies a Program
label such as `Stellar Wave`, and an issue can also be added to a Program simply
by applying that label yourself.

The `drips:1`, `drips:3`, `drips:5` and `drips:8` labels that exist in this
repository are a **local convention predating the Wave application**. They are a
rough indication of size for humans reading the issue list. They do **not**
correspond to the Point values above and do not determine what a merged pull
request earns. Do not read a `drips:N` label as a payment.

## Labels that exist

Only these are defined in this repository today:

| Label | Use it for |
| --- | --- |
| `drips:1` `drips:3` `drips:5` `drips:8` | rough size hint for humans, see above |
| `bug` | Something is wrong that you can demonstrate. |
| `documentation` | Documentation only, no behaviour change. |
| `scripts` | Shell scripts and tooling under `scripts/`. |
| `testing` | Tests and test infrastructure. |
| `security` | Security analysis or hardening. |
| `contracts` | Touches Soroban contract code. |
| `good first issue` | Small, well-scoped, safe for a newcomer. |
| `help wanted` | A maintainer would like outside help. |

`bug`, `duplicate`, `enhancement`, `invalid`, `question`, `wontfix` and
`accessibility` are GitHub's built-in defaults.

Labels mentioned in older issues that do **not** exist here — `audit`, `spec`,
`docs`, `blocked` — are still referred to in a few places. Treat them as prose.
If you need one, say so on the issue and a maintainer can add it.

## Pull requests

- One logical change per PR. Match the repo's commit style
  (`type(scope): imperative summary`, e.g. `feat(waterfall): settle at maturity`).
- Say which invariant or spec clause your change is anchored to. If you
  cannot, it is probably out of scope for the PR.
- Add a test that fails without your change. For arithmetic, prefer a property
  test over a table of hand-picked cases.
- If you touch `contracts/waterfall`, expect a review comment on your
  property tests. Proving an invariant is the whole point.

## CI

Five jobs run on every push to `main` and every pull request. All five must pass.
These are the exact job names, as they appear in the checks list:

| Job name | What it does |
| --- | --- |
| `fmt and clippy` | `cargo fmt --all -- --check` and `cargo clippy --all-targets --all-features -- -D warnings` |
| `test` | `cargo test --all --locked`, then the whole suite again in release with `PROPTEST_CASES=20000` |
| `build wasm` | installs the Stellar CLI, runs `stellar contract build`, and asserts that **both** `strata_epoch_manager.wasm` and `strata_mock_vault.wasm` exist and are non-empty |
| `docs are current` | every spec invariant has a matching property test, the spec's own links resolve, and `scripts/deploy-testnet.sh --self-test` proves the mainnet guard still refuses |
| `dependency audit` | `cargo audit` |

`dependency audit` is currently `continue-on-error`, so it reports without
blocking a merge. Treat a new advisory there as something to fix rather than
ignore.

Run the same checks locally before opening a PR:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --locked
PROPTEST_CASES=20000 cargo test --release --locked
./scripts/deploy-testnet.sh --self-test
```

## Stellar CLI problems

If a `stellar contract invoke` **passes simulation and then fails on submission**
with `storage exceeded_limit` and a message about accessing a contract data key
"outside of the footprint", add `--no-cache` and run it again.

The CLI caches simulations, and a cached one can carry a footprint that no longer
covers the current ledger state. This was hit on testnet with CLI 28.1.0 during
the original deployment; it is a CLI problem, not a contract bug, and the
transaction is correctly refused rather than silently mis-executed. See
[`docs/deployment.md`](docs/deployment.md#known-issues).

Testnet's public RPC also drops connections intermittently, which surfaces as
`client error (SendRequest)`. Nothing happened; re-run the command.

Every script in `scripts/` passes `--no-cache` throughout for this reason.

## Reporting a security issue

Do **not** open a public issue for an unresolved vulnerability. See
[SECURITY.md](SECURITY.md). Note that this project is unaudited: treat any
funds at risk as your own risk.

## License

By contributing you agree that your work is licensed under
[Apache-2.0](LICENSE), the same terms as the rest of the project.