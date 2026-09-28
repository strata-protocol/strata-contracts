# Risk register

**Strata is unaudited and testnet-only. Do not treat anything here as
mitigated.**

This is a working list, not a completed audit. Each entry says what the risk
is, what the code does about it *today*, and what is still open. Risks that
have caused a bug during development are marked **[realised]** — those are not
hypothetical, they shipped and were caught by a test.

Severity is about the unaudited, testnet context: **High** would mean loss of
funds or a broken settlement, **Medium** would mean a depositor can be harmed
in a plausible scenario, **Low** is a correctness or operability wart.

---

## Contract and settlement

### R1 — Unaudited settlement math — **High**

A bug in the waterfall misallocates real value. Property tests cover five
invariants, but the space of inputs and of *state* transitions around the
waterfall is not exhausted.

*Mitigation:* the math is a pure `i128` function with no storage, no `Env` and
no SDK dependency, so it is small enough to read in full and is tested
exhaustively within its documented domain. Invariants 1–5 each have property
tests. Bounds (`MAX_PRINCIPAL`, rate, term, ratio) are derived from the
overflow headroom rather than chosen by feel.

*Open:* no external review. No formal verification. No independent
reimplementation to diff against.

### R2 — Integer truncation in the interest term — **Low, by design**

`(S * r * t) / (10_000 * SECONDS_PER_YEAR)` truncates. The discarded fraction
accrues to the junior, so the protocol never overpays the senior.

*Open:* nothing. Documented in spec §4, and
`invariant_1_survives_interest_truncation` exists to keep it that way.

### R3 — The cushion is `J - I`, not `J` — **Medium, documented**

A depositor reading "the junior buffer protects the senior" may assume the
senior is safe against losses up to the full junior principal. It is not. The
senior is made whole only while total loss `<= J - I`, where `I` is the
senior's own target interest. A buffer exactly equal to `I` gives a cushion of
zero.

*Mitigation:* stated in spec §3, in a table in §9, and pinned by
`the_cushion_boundary_is_sharp`.

*Open:* it is an economically surprising property. If it turns out to be the
wrong design, changing it is a **spec change** and needs maintainer sign-off.

### R4 — A senior depositor can lose their entire principal — **Medium**

`senior_payout` floors at `V`, and `V` floors at zero. There is no floor under
senior principal. That is what "senior" means here, but it is a real risk to
anyone who reads "senior" as "safe".

*Open:* nothing mitigates it. It is inherent to the design and is what the
junior buffer exists to soften.

### R5 — Pooled yield, not per-depositor yield — **Medium**

Deposits enter the vault immediately and the manager holds one pooled position.
Yield earned by one depositor's capital is shared by the whole pool according to
the waterfall, rather than accruing to whoever was invested at the time.

*Consequence:* a depositor who enters late in a high-yield window and exits
early participates in gains they did not generate and in losses they did not
suffer, pro-rata by principal.

*Open:* not addressed. A per-depositor cost basis in the vault would be a
significant redesign. Worth deciding deliberately before there is real money.

---

## The underlying vault

### R6 — Strata trusts the vault completely — **High**

`total_assets`, `convert_to_assets` and the redemption return value all come
from the vault. A vault that lies about its assets causes an incorrect
settlement.

*Mitigation, and its limits:* `settle` uses the amount actually **returned by
`redeem`**, never a reported figure, and re-reads `balance_of` to check the
position before redeeming. It also verifies the manager actually holds
`senior_payout + junior_payout` before promising anyone any of it.

*Open:* a vault can still return less than its own reported assets, and Strata
has no way to distinguish that from a real loss. The checks catch
*mismatch*, not a vault that is consistently dishonest. Wrapping an untrusted
vault is out of scope and should be assumed impossible.

### R7 — The mock vault's yield is unbacked — **Low, test-only**

`mock-vault` accrues accounting yield with no strategy behind it. Tests
pre-fund a "strategy reserve" so redemptions can settle.

*Mitigation:* documented in the crate docs and at the top of the integration
tests. The consequence is that the mock's *real token balance* is not a
meaningful assertion target; `total_assets` and redemption returns are.

*Open:* a new contributor could write a test asserting on the mock's token
balance and get a confusing result. The docs say so; nothing enforces it.

### R8 — A vault that reverts blocks settlement — **Medium**

If the vault's `redeem` reverts — paused, broken, over its gas budget — the
epoch cannot settle and depositor funds are stuck in the vault for as long as
that persists.

*Mitigation:* `settle` is permissionless, so **anyone** can retry it. There is
no admin key that can block or reorder settlement.

*Open:* there is no timeout, no emergency path, and no alternative settlement
route. A permanently broken vault means permanently locked funds. Recorded
here as an accepted, unresolved risk of the design.

---

## State and operations

### R9 — Storage expiry — **Medium**

A Soroban persistent entry that expires is *silently missing*, not an error.
Losing a `Position` entry would make a depositor's principal unreadable and
their claim impossible.

*Mitigation:* every write bumps TTL with a long `BUMP_AMOUNT`. Positions are
written on deposit and again on claim, and the epoch entry is written on every
lifecycle transition.

*Open:* TTL is only extended when an entry is *touched*. A depositor who
deposits and never claims has their position bumped at deposit time and then
left alone for the length of `BUMP_AMOUNT`. If `BUMP_AMOUNT` in ledgers is
shorter than the time to claim, the position can expire. **This needs a
deliberate check against realistic ledger timings and a keeper or crontab if
the numbers are close.** Not yet done.

### R10 — The admin can choose epoch terms — **Medium**

`create_epoch` lets the admin pick the term, the senior target rate and the
senior/junior ratio. A malicious or compromised admin can open an epoch with
an attractive rate that the underlying will not earn, and depositors who trust
the terms lose money.

*Mitigation:* every parameter is bounded by the documented limits, and the
ratio can never exceed 1.0x, so the junior buffer is always at least as large
as the senior principal. The admin cannot set a rate above 100%/yr or a term
beyond one year.

*Open:* the admin still has meaningful discretion, and depositors must trust
it. A rate high enough to look attractive while the buffer ratio looks safe is
the obvious attack, and the bounds do not prevent it. Multisig or timelock on
the admin is the obvious mitigation and is **not implemented**.

### R11 — No deposit cap per depositor — **Low**

The only limit on a depositor is `MAX_PRINCIPAL` and the senior ratio gate. A
single depositor can take the entire senior side of an epoch.

*Open:* not a bug, but it means depositors cannot assume diversification.

---

## Code-level risks that actually materialised

### R12 — **[realised]** Unbacked pro-rata denominator — **High**

`claim` originally divided by the *unclaimed* principal, which shrinks as
people claim. The first claimant received a correct slice, the second received
a larger one, and the tranche would have been overpaid. Caught by the
cross-contract pro-rata test before any deployment.

*Now:* the denominator is the tranche's total principal, which never changes.
Pinned by `several_senior_depositors_split_pro_rata_and_the_total_is_exact`.

### R13 — **[realised]** A wrong overflow bound — **High**

`MAX_ASSETS` in `mock-vault` was written as 1e27 while its documentation
claimed 1e24. At 1e27 the accrual product overflows `i128`, so the entire
overflow argument for the mock was false. Nothing detected it: the constant was
a literal in a doc comment nobody had recomputed.

*Now:* the value is correct and `the_asset_ceiling_keeps_accrual_inside_i128`
asserts the arithmetic rather than asserting it in prose.

*Lesson applied:* derived bounds are now tested as arithmetic, not documented as
arithmetic. The same treatment is given to `MAX_PRINCIPAL` in
`invariant_5_never_panics_at_the_extreme_corner`.

### R14 — **[realised]** An unreachable bootstrap branch — **Medium**

`mock-vault`'s first-depositor case was dead: the zero-assets check ran before
the zero-supply check, and on a first deposit both are zero, so the very first
depositor received zero shares for a real token transfer.

*Now:* the ordering is documented as load-bearing, and the first-depositor and
wiped-vault cases are both pinned.

### R15 — **[realised]** Missing authorisation entry — **Medium**

The manager→vault→token call chain needs an explicit
`authorize_as_current_contract` entry, because the token call is two frames
below the manager. Written correctly, but verified only by `mock_all_auths`,
which passes whether or not the entry is right.

*Now:* `the_depositor_authorises_the_deposit_with_the_right_arguments`
inspects the auth tree. This is the one place in the codebase where
`mock_all_auths` is insufficient and the tests say so.

---

## Process and supply chain

### R16 — SDK version coupling — **Medium**

Soroban contract binaries are only portable within a single SDK major. A
`Cargo.lock` update can silently change behaviour.

*Mitigation:* one `soroban-sdk` version in `[workspace.dependencies]`, bumped
alone in its own commit. `overflow-checks = true` in every profile, mandatory
per CVE-2026-24889, with a test that fails if it is ever disabled.

*Open:* no `cargo audit` in CI. No dependency review.

### R17 — No mainnet guardrail exists yet — **Medium**

This repository is testnet-only by policy. Policy is not a control.

*Open:* the deploy script that would refuse a mainnet passphrase does not exist
yet. It must, before anything is deployed anywhere.

### R18 — Unaudited claims in the README — **Low**

The README must not imply the contracts have been reviewed.

*Mitigation:* the disclaimer is the first thing in the README and in every
document.

---

## Not yet written down

Honest list of risks I have not yet analysed, so they are absent above rather
than dismissed:

- Gas and resource limits on mainnet. Tests enforce mainnet resource limits by
  default from SDK v25, and nothing currently fails, but a worst-case
  `settle` with many depositors has not been measured.
- Reentrancy beyond the claim path's position-clearing. The mock vault calls
  back into nothing, so a hostile vault's callback behaviour is untested.
- Front-running settlement. Anyone can call it, and the result depends on the
  vault's valuation at that moment. A vault whose price can be moved in the
  same block makes this a real vector.
- Formal verification of the `mul_div` rounding direction against the
  last-claim remainder, beyond the property tests.
- The economics of `MAX_SENIOR_RATIO_BPS` against realistic vault volatility.
  The constant is safe; whether 1.0x is *sufficient* is a modelling question
  nobody has answered.
