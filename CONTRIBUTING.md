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
git clone https://github.com/strata-finance/strata-contracts
cd strata-contracts
rustup show                 # rust-toolchain.toml pins the toolchain + wasm target
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Install the Stellar CLI to build deployable Wasm. `cargo build` will **not**
produce a usable contract; the Soroban runtime only accepts `wasm32v1-none`
builds produced by `stellar contract build`.

```bash
cargo install stellar-cli --locked
stellar contract build      # -> target/wasm32v1-none/release/*.wasm
```

## Issue labels

| Label | Use it for |
| --- | --- |
| `drips:1` | Small: typo, doc fix, added test, one-line clarity change. |
| `drips:3` | Medium: a bounded feature or bug fix touching one crate. |
| `drips:5` | Large: multi-crate feature, or a change to settlement-adjacent code. |
| `drips:8` | Complex: anything touching the waterfall spec, auth model, or storage layout. |
| `bug` | Something is wrong that you can demonstrate. |
| `audit` | Needs a second pair of eyes before merge. |
| `spec` | Concerns `docs/waterfall-spec.md`. Requires maintainer sign-off. |
| `docs` | Documentation only, no behaviour change. |
| `good first issue` | Small, well-scoped, and safe to hand to a newcomer. |
| `blocked` | Waiting on an upstream or a maintainer decision. |

### Wave points

Issues carry a `drips:N` label, and that number is the **Wave point value** of
the work: roughly how many Wave points a merged PR earns.

- `drips:1` — **1 Wave point.** A few lines. No new design surface.
- `drips:3` — **3 Wave points.** A real change inside one crate, with tests.
- `drips:5` — **5 Wave points.** Cross-crate work, new storage, new events, or a
  new error variant.
- `drips:8` — **8 Wave points.** Cross-cutting or security-sensitive: auth,
  settlement math, or anything requiring a spec decision.

If an issue turns out to be bigger than its label, say so in the thread and
relabel before starting rather than after.

## Pull requests

- One logical change per PR. Match the repo's commit style
  (`type(scope): imperative summary`, e.g. `feat(waterfall): settle at maturity`).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test` must all pass locally. CI runs the same three.
- If you touch `contracts/waterfall`, expect a review comment on your
  property tests. Proving an invariant is the whole point.
- Say which invariant or spec clause your change is anchored to. If you
  cannot, it is probably out of scope for the PR.
- Add a test that fails without your change. For arithmetic, prefer a property
  test over a table of hand-picked cases.

## Reporting a security issue

Do **not** open a public issue for an unresolved vulnerability. See
[SECURITY.md](SECURITY.md). Note that this project is unaudited: treat any
funds at risk as your own risk.

## License

By contributing you agree that your work is licensed under
[Apache-2.0](LICENSE), the same terms as the rest of the project.
