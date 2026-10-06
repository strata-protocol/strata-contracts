# Undisclosed Vulnerability Report

Strata is **unaudited** and testnet-only. We still want to hear about real
bugs, especially in the settlement math.

## Do not open a public issue

If you believe you have found a vulnerability that is not yet fixed, report it
privately. A public issue gives attackers a head start and tends to make a
partial fix look complete.

## How to report

Email **sulaimonifeoluwa4@gmail.com** with the details below. Prefer a private
GitHub security advisory if one is available on the repository; otherwise email.

> Note: GitHub private vulnerability reporting is currently **disabled** on this
> repository (verified against the API on 2026-10-06), so the "Security →
> Report a vulnerability" button does not appear and there is no GitHub-tracked
> private thread. The email address above is the channel. It is a personal
> address, not an organisation one — if that matters to you, `TODO(maintainer)`
> to move it behind an org alias or enable private reporting.

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
