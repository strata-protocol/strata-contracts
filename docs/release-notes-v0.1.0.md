# Strata v0.1.0 — release notes (DRAFT)

> **This is a draft.** The tag and the GitHub release have deliberately **not**
> been created. Both are a maintainer's job after review. The exact command is at
> the bottom.

> ## Unaudited. Testnet-only. Do not put real funds into it.
>
> Nothing in this release has been reviewed by a third party. There is no
> external audit, no formal verification, and no independent reimplementation to
> diff the settlement math against. The arithmetic is property-tested and its
> bounds are derived rather than guessed, but that is not an audit and not a
> security review. See [`docs/risks.md`](risks.md) for what is known to be
> unresolved, including four bugs that shipped during development.

---

## What this is

A tranche wrapper on Stellar Soroban. Strata wraps an ERC-4626-style vault and
splits the result of one fixed-term epoch into two tranches:

- **Senior** — principal plus a fixed target rate, capped at that target, paid
  first.
- **Junior** — everything above the senior target, absorbing losses first.

This release contains the contracts, the tests, the spec they implement, and a
**live testnet deployment** with two real settled epochs on the record.

---

## Release contents

| Crate | Purpose |
| --- | --- |
| `contracts/waterfall` | Pure `i128` settlement math. No `Env`, no storage, no `soroban-sdk` dependency at all. |
| `contracts/vault-interface` | The ERC-4626 subset Strata needs, so the manager depends on an interface rather than a vault. |
| `contracts/epoch-manager` | The contract. Owns the money and the state machine. |
| `contracts/mock-vault` | Test double with an admin-set signed yield rate, negative included. Testnet only. |
| `tests/` | The five spec invariants as 16 property tests, plus 30 cross-contract integration tests. |

## Deployed contracts

**Network:** Stellar `testnet` — `Test SDF Network ; September 2015`, protocol 29
**Deployed:** 2026-10-05T11:35:50Z
**Stellar CLI:** 28.1.0 **soroban-sdk:** 27.0.6

| Contract | Contract ID | Wasm sha256 | Bytes |
| --- | --- | --- | --- |
| `epoch-manager` | `CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU` | `b12c8af8403df7478ee67f4e6dd292230236f086a4908894ef80d19417828f7a` | 25 964 |
| `mock-vault` | `CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6` | `a3599c7022dc90c5df2fe009e49e25de6fb087536f872c588efc5cc732346759` | 9 638 |
| Native XLM SAC (underlying) | `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC` | builtin | — |

Admin: `GBIMKBYJVP3VNFJPU45XMGBKHLESO5QLUYWAAMMRMGTPDF6NUD5OWRIP`

- Explorer: [epoch-manager](https://stellar.expert/explorer/testnet/contract/CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU) · [mock-vault](https://stellar.expert/explorer/testnet/contract/CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6)
- Full record: [`deployments/testnet.json`](../deployments/testnet.json)
- Full write-up: [`docs/deployment.md`](deployment.md)

**The on-chain Wasm hash of each contract was checked against the sha256 of the
locally built artefact and matched exactly**, so the deployed code is the code in
this repository. Nothing binds it to a commit — see R17a in
[`docs/risks.md`](risks.md).

## Test status

**104 tests, all passing**, counted by `cargo test --all --locked` as one
`#[test]` per case: 19 in `contracts/waterfall`, 21 in `contracts/mock-vault`, 18
in `contracts/epoch-manager`, and 46 in `tests/` — split 16 property tests over
the five spec invariants and 30 cross-contract integration tests.
`contracts/vault-interface` has none; it is a trait declaration. `cargo fmt
--check` and `cargo clippy -- -D warnings` are clean. CI runs fmt, clippy, the
suite, a 20 000-case property-test pass, a real `stellar contract build` with
artefact checks, a spec consistency check, and `cargo audit`.

**Two real epochs were run on testnet and settled**, with every figure checked
against [`docs/waterfall-spec.md`](waterfall-spec.md) recomputed independently in
the script rather than read back from the contract:

| Scenario | `V` | `senior_payout` | `junior_payout` | Demonstrates |
| --- | --- | --- | --- | --- |
| `good` | 2 000 204 528 | 1 000 009 512 | 1 000 195 016 | the cap binding: the senior took its 9 512 target, the junior took the other 195 016 |
| `loss` | 1 999 836 692 | 1 000 009 512 | 999 827 180 | the senior made whole, the junior absorbed the whole 172 820 loss |

In both, `senior_payout + junior_payout == V` to the stroop, and each tranche
paid out exactly its own payout across the claims. The deposit gate was verified
by observing a senior deposit one stroop past the cap rejected with
`SeniorGateViolated`. The cushion boundary was verified through the contract's
own `project` view at break-even, at the limit, one unit past it, and at total
wipeout.

Transaction hashes: [`docs/deployment.md`](deployment.md).

## Known limits of what was verified

- **No cushion-breaching loss was settled.** Reaching one needs about 18.25 days
  at the contract's own maximum rate and term bounds. The bounds are correct and
  were not changed; the boundary was shown via `project` instead.
- **The demo's gains and losses are small in absolute terms.** A 300-second term
  at maximum annualised rates moves the vault by a fraction of a percent. Exact,
  but not dramatic.
- **TTL is extended but not audited.** R9 is open: a depositor who deposits and
  never claims has a position bumped only at deposit time.
- **`mock-vault`'s yield is unbacked** and must be pre-funded to pay a
  redemption. R7, test-only by construction.
- **Testnet is not durable.** SDF resets it periodically, which clears these
  contract IDs.
- **Two stray test vault contracts exist from debugging the deployment**, are
  recorded in [`docs/deployment.md`](deployment.md), and are inert.

## What is not done

- **External audit.** Not requested. R1, R6 and R8 are the ones an audit would
  most need to look at.
- **TTL audit for per-user positions (R9).** Open, and the one operational risk
  most likely to cost someone money.
- **Multisig or timelock on the admin (R10).** The admin can pick epoch terms,
  and depositors must trust that choice. Not mitigated.
- **`strata-app` SDK and dashboard.** Separate repository, not started. Needs a
  pinned contract version from this release.
- **Per-depositor cost basis in the vault (R5).** Yield is pooled, not
  per-depositor. A real economic characteristic, deliberately not redesigned here.
- **Mainnet support.** Out of scope, deliberately and permanently. This project
  will not do it.
- **Reproducible or verifiable deployment (R17a).** No signature or attestation
  ties a running contract to a reviewed commit.

## The design in one table

From the spec, `S = J = 1_000_000`, senior target 5%/yr, one-year term, so
`senior_due = 1_050_000`:

| `V` | `senior_payout` | `junior_payout` | what happened |
| --- | --- | --- | --- |
| 2 200 000 | 1 050 000 | 1 150 000 | healthy; senior capped, junior takes the rest |
| 1 100 000 | 1 050 000 | 50 000 | a 900 000 loss, fully absorbed by the buffer |
| 1 049 999 | 1 049 999 | 0 | one unit past the cushion; the senior starts paying |
| 900 000 | 900 000 | 0 | junior wiped, the senior covers the shortfall |
| 0 | 0 | 0 | total wipeout |

`senior_payout + junior_payout == V` always. The senior never beats its target.
The junior is paid nothing unless the senior was made whole first.

The cushion is `J - I`, where `I` is the senior's *own* target interest — not
`J`. A buffer exactly equal to `I` leaves a cushion of zero. This is the single
most misreadable property of the design and it is called out in
[the spec](waterfall-spec.md#why-these-bounds) and in [risks.md](risks.md).

---

## For the maintainer: creating the release

Not run. Not to be run before review and after the PR is merged.

```bash
# 1. Merge the PR first. This release notes file ships inside the repo, so it
#    must be on the default branch before the tag.
#    (https://github.com/strata-protocol/strata-contracts/pulls)

# 2. Confirm the branch you are on is the default branch and is up to date.
git checkout main
git pull --ff-only
git log --oneline -1

# 3. Confirm the suite is green locally before tagging.
cargo test --all --locked
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
stellar contract build

# 4. Tag. Annotated, signed if your key is set up.
git tag -a v0.1.0 -m "Strata v0.1.0 - unaudited testnet release"
git push origin v0.1.0

# 5. Create the release from the tag, using these notes.
gh release create v0.1.0 \
  --title "Strata v0.1.0 (unaudited, testnet only)" \
  --notes-file docs/release-notes-v0.1.0.md
```

Before running step 5, two things are a maintainer's decision and not an agent's.
Both are about whether this release should exist at all in this shape.

**1. Whether to tag an unaudited contract suite as `v0.1.0` at all.** Options:
tag it and let the title and this file carry "unaudited, testnet only" in
everywhere they appear; or hold the tag until the external audit exists, which
would mean the release notes describe a deployment nobody can install. The
second is slower and safer; the first is what the current text assumes.

**2. The status of the TTL audit (R9) and the external audit.** Both are listed
as open above. If either has moved by release time, this file is wrong and should
be corrected rather than the risk dismissed. In particular, R9 is the one open
Medium risk where an uncomputed number is load-bearing for whether a depositor
can still get their money back — see issue #10.

(The repository and organisation references in this file were previously
inconsistent with the checkout's `origin` remote. The repository has since been
transferred to `strata-protocol/strata-contracts` and every reference now agrees,
so that item is closed.)

`strata-app` is referenced above as the SDK and dashboard. It does not exist yet,
so nothing in this release depends on it.

Verified against the chain on 2026-10-06, read-only: both contracts are still
live, and the on-chain Wasm hash of each still matches the hash recorded in
`deployments/testnet.json`. The recorded test counts were re-derived from
`cargo test --all --locked` for this release.
