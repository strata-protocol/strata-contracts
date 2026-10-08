# Security policy

Strata is **unaudited** and testnet-only. We still want to hear about real
bugs, especially in the settlement math.

## Do not open a public issue

If you believe you have found a vulnerability that is not yet fixed, report it
privately. A public issue gives attackers a head start and tends to make a
partial fix look complete.

## How to report

Report it privately through GitHub:

1. Go to the **Security** tab of this repository.
2. Click **Report a vulnerability**.
3. Fill in the form.

If you cannot use GitHub's private reporting, email
**sulaimonifeoluwa4@gmail.com** instead.

Please include:

- what an attacker can do, and who would be affected,
- affected commit, release tag or contract version (the wasm hash or contract
  ID, if you have it),
- the exact inputs: `S`, `J`, `r`, `t`, `V`, and the epoch configuration,
- the observed behaviour and the behaviour you expected,
- whether the five invariants in [`docs/waterfall-spec.md`](docs/waterfall-spec.md)
  still hold under your case,
- a minimal test that reproduces it, ideally a proptest.

## What to expect

- We aim to acknowledge reports within 7 days and to send a status update
  within 14 days.
- A decision on whether the report is valid, and an estimate for a fix where
  we can give one.
- Credit in the fix commit unless you prefer otherwise.

Strata is unaudited, testnet-only software maintained by a small team, so we
cannot promise a fix timeline.

## Scope

In scope: the contracts in this repository, the settlement math, the auth and
gating model, and the deploy scripts.

Out of scope: the Stellar protocol itself, `soroban-sdk` (report upstream),
and anything about mainnet deployment — this project does not deploy to
mainnet, so there is no mainnet loss to investigate.

Also out of scope:

- Risks already listed in `docs/risks.md`, unless you have a new reproduction
  or a change in severity.
- Issues in the dashboard or SDK. Those belong in the **strata-app**
  repository's Security tab.
- Testnet resets and RPC outages.
