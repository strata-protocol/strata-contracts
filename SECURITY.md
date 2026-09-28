# Undisclosed Vulnerability Report

Strata is **unaudited** and testnet-only. We still want to hear about real
bugs, especially in the settlement math.

## Do not open a public issue

If you believe you have found a vulnerability that is not yet fixed, report it
privately. A public issue gives attackers a head start and tends to make a
partial fix look complete.

## How to report

Open a **private** security advisory on this repository
("Security" -> "Report a vulnerability"). If advisories are disabled, ask a
maintainer for a direct contact via the org's public profile and do not post
the details anywhere public.

Please include:

- affected commit or contract version (the wasm hash, if you have it),
- the exact inputs: `S`, `J`, `r`, `t`, `V`, and the epoch configuration,
- the observed behaviour and the behaviour you expected,
- whether the five invariants in [`docs/waterfall-spec.md`](docs/waterfall-spec.md)
  still hold under your case,
- a minimal test that reproduces it, ideally a proptest.

## What to expect

- Acknowledgement within a few days.
- A decision on whether the report is valid, and an estimate for a fix.
- Credit in the fix commit unless you prefer otherwise.

## Scope

In scope: the contracts in this repository, the settlement math, the auth and
gating model, and the deploy scripts.

Out of scope: the Stellar protocol itself, `soroban-sdk` (report upstream),
and anything about mainnet deployment — this project does not deploy to
mainnet, so there is no mainnet loss to investigate.
