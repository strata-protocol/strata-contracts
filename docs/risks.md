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

### R19 — A deposit in the final ledger takes a full term of the waterfall — **Medium**

Deposits stay open while `now < maturity_ts`, but the senior target is computed
on the senior principal and the epoch's fixed term, with no time-weighting. A
senior depositor who enters in the last ledger before maturity is paid a whole
term's target out of `V`, and the difference comes from the junior. Measured at
10%/yr with 400 000 junior and 200 000 senior: a second senior depositing
200 000 one second before maturity claims 220 000, while the junior receives
419 999 instead of 440 000 — 20 000 for one ledger of exposure. The junior side
is the same mechanism through R5's pooling, ungated: a junior arriving in the
final second took 20 001 of yield from the junior who funded the term. The vault
share price is not the cause (the late deposit buys shares at the correct
pre-deposit price), so this cannot be fixed in the vault.

*Bound:* per epoch, by the senior room under the ratio gate and by the rate. The
late depositor must leave the principal until settlement, and `close_epoch`
keeps the manager to one epoch at a time, but there is no holding period, no
per-depositor cap and no time-weighting.

*Open:* no fix. Time-weighting the senior target, closing deposits earlier than
settlement, or a minimum holding period are each a spec or design change. Filed
as issue #32; measured in `tests/src/settlement_ordering.rs` and
[analysis/settlement-ordering.md](analysis/settlement-ordering.md).

That analysis also covers settlement timing and claim order: settlement pays its
caller nothing, and settling thirty days late moves only the junior (+4 931 at
+10%/yr, −4 931 at −10%/yr; the senior is unchanged at 220 000). Claim order
decides one stroop of remainder and nothing else. The vector this design does
not bound is `V` itself: a vault whose price can be moved in the settlement
ledger lets whoever moves it choose the valuation (R6, B1, B2). For the mock
vault the only mover is the admin (R10), and a final-ledger rate change is worth
one stroop of truncation.

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
no admin key that can block or reorder settlement. Pinned by
`vault_trapping_on_deposit_or_redeem_rolls_back_and_allows_retry_when_fixed` in
`tests/src/hostile_vault_test.rs`.

*Open:* there is no timeout, no emergency path, and no alternative settlement
route. Because `Config.vault` is immutable once constructed, a permanently
broken vault means permanently locked funds unless the vault contract itself is
upgraded or unpaused by its own administrator. Recorded here as an accepted,
unresolved risk of the design.

### R8a — Hostile-vault reentrancy and malformed vault callbacks — **Low (reentrancy) / High (dishonest partial redemption), analysed**

`epoch-manager` invokes the external vault during `deposit` (`vault.deposit`),
`settle` (`vault.balance_of`, `vault.redeem`) and the read-only `position_of`
view (`vault.convert_to_assets`). `tests/src/hostile_vault_test.rs` drives a
malicious `HostileVault` across every callback and fault mode:

| Vector | Vault behaviour | Observed outcome | Can funds become stuck & who can recover? |
| --- | --- | --- | --- |
| Re-enter `deposit`, `settle` or `claim` from `vault.deposit` | Calls back into `epoch-manager` during `deposit` | **Rejected by Soroban host** (`InvokeError::Abort`); transaction rolls back atomically ([Soroban security docs](https://developers.stellar.org/docs/build/security-docs)) | No — depositor keeps 100% of tokens |
| Re-enter `deposit`, `settle` or `claim` from `vault.redeem` | Calls back into `epoch-manager` during `settle` | **Rejected by Soroban host** (`InvokeError::Abort`); epoch remains `Open` and `vault_shares` unchanged | Only while vault remains malicious; recoverable once vault stops re-entering and anyone retries `settle()` |
| Inflated or deflated `total_assets` / `convert_to_assets` | Reports 100x or 0 on view calls | Pre-settlement `position_of().estimated_payout` reflects the lie, but **`settle()` ignores both views** and settles strictly on `redeem` output | No — settlement and claims pay exact waterfall amounts |
| Inflated `redeem` return or underpaid token transfer | Returns `value > transferred` so `owed > held` | **Rejected by manager** (`Error::InvalidAmount` at `held < owed`); `settle()` rolls back | Funds remain in vault with epoch `Open` until vault transfers at least the reported `value` |
| Dishonest partial `redeem` | Transfers and reports `0.5x` while keeping `0.5x` in vault | **Succeeds** (`held == owed`); manager treats shortfall as a strategy loss (R6), Junior absorbs first, and `epoch.vault_shares` is zeroed | **Yes** — the withheld `0.5x` remains stranded inside the vault contract; only the vault's own admin/governance could recover it |
| Vault traps (`panic!`) on `deposit` or `redeem` | Traps inside `deposit` or `redeem` | `deposit` rolls back leaving tokens with depositor; `settle` rolls back leaving epoch `Open` (R8) | On `redeem` trap, funds stay locked in the vault until the vault stops trapping and `settle()` is retried |

---

## The Blend adapter

Risks B1–B7 were identified by the research in
[design/blend-adapter.md](design/blend-adapter.md) §8. They are recorded here now
that there is code behind them. The adapter exists, is tested against a test
double, and has been deployed against the real testnet Blend pool, where it
settled one 300-second epoch ([deployment.md](deployment.md)). One epoch is
plumbing, not evidence about liquidity, losses or a hostile pool, so nothing here
is mitigated in the sense a depositor would need.

### B1 — The Blend pool is a third party — **High, open**

The adapter delegates custody to somebody else's contract, its admin, its mock
oracle and its backstop. A bug in the pool, a freeze, a compromised admin or a
bad-debt event all reach Strata depositors directly.

*Mitigation:* none, and none is possible from inside Strata. The adapter is
narrow — it can only supply, withdraw and read — and it has no admin that could
redirect the position. That bounds what Strata can do to the pool, not what the
pool can do to Strata.

*Open:* everything. Whether a real Blend pool is an acceptable underlying is a
maintainer decision that has not been made.

### B2 — Liquidity at settlement — **High, open**

Settlement redeems the entire position at one moment. If the pool cannot pay —
utilisation at or near 100%, or a reserve the pool has frozen — the redemption
reverts and the epoch cannot settle.

*Mitigation:* none. This is R8 with a real cause behind it: the money may be lent
out to borrowers who have not repaid. `settle` is permissionless, so anyone can
retry, but retrying does not create liquidity.

*Open:* there is no partial settlement and no timeout. A pool that stays fully
utilised means funds wait. The manager-level failure and retry are exercised
end to end against the pool double
(`a_liquidity_limited_redeem_blocks_settlement_and_can_be_retried`), so the
state machine's half is known; whether a real pool has the liquidity remains
unknown.

### B3 — A silent partial withdrawal — **Medium, mitigation built, not exercised by a real clamp**

Blend clamps an over-sized withdrawal instead of rejecting it: it burns what
there is, pays what that is worth, and reports success. A vault that trusted the
return value would under-pay settlement with no error anywhere.

*Mitigation:* the adapter measures its own token balance across the withdrawal
and traps with `InsufficientWithdrawal` if less arrived than it asked for, which
rolls the whole invocation back. `a_short_payment_from_the_pool_is_refused`
drives this using a *deliberately unfaithful* pool double, because a correct
adapter asking for exactly what the pool reported can never trigger the real
clamp. The double's clamp itself is tested in `blend-pool-mock`.

*Open:* the check ran during one real redemption on testnet (2026-11-05) and did
not fire, because the pool paid in full; whether Blend's clamp behaves as read on
a network is still unproven. A clamp that lands on exactly the computed amount
would also be indistinguishable from success. The property tests in
`tests/src/blend_adapter_props.rs` pin the adapter's side of the arithmetic
against the faithful double — neither a deposit nor a redemption creates value
through rounding — but they cannot test the real clamp.

### B4 — The Blend position can expire — **High, open**

An idle Blend user entry expires after roughly 120 days and then reads back as an
**empty position, not an error**. Strata's maximum term is 365 days, so an epoch
can outlive the position its settlement depends on.

*Mitigation, and only half of one:* the adapter records the last position the
pool reported and refuses to value or redeem a position the pool has stopped
recognising, so the failure is loud (`PositionMissing`) rather than a silent
zero. That detects the loss; it does not prevent it.

A permissionless `refresh()` now reads the pool's `get_positions`, the call that
renews the pool's own entry, and records the ledger in `last_refreshed`;
`deposit` and `redeem` go through the same read. It cannot extend the entry
itself — the entry belongs to the pool — so the pool's own read is the
mechanism.

*Open:* the extension has **not** been observed. On 2026-11-05 a real adapter
position was refreshed on testnet from an identity that was not the manager: the
call succeeded, `last_refreshed` moved 5 569 344 → 5 569 359, and the pool
entry's live-until ledger stayed at 7 642 944 before and after. The entry was
created with 2 073 600 ledgers (120.0 days) of life and still had ~119 days left
at the refresh, above Blend's ~100-day extension threshold, so the read was a
no-op by design. The measured lifetime of a fresh entry is 120.0 days; a safe
epoch length longer than that remains unproven, and a keeper is still required.
Re-running the measurement on an entry within ~100 days of expiry is what would
close the proof gap.

### B5 — Rounding dust — **Low, open**

Supplying floors the bTokens minted and withdrawing ceilings the bTokens burned,
so a round trip leaves a few base units behind. On the design note's testnet
measurement, a 100 XLM round trip left 60 bTokens (about 148 stroops).

*Mitigation:* none needed for correctness — the dust accrues to remaining
holders rather than being lost — but the adapter's accounting must never assume
shares in equals shares out, and it does not: it reads the pool's own reported
position after every call rather than tracking a running total.

*Open:* with one adapter position per epoch and no per-depositor shares at the
pool, the dust is donated to whoever deposits next. That is a fairness wart, not
a leak, and it is not measured on a real pool. Property tests now pin the two
bounds that keep it a wart: a deposit cannot add more value than it paid, and a
redemption cannot leave more value behind than it started with.

### B6 — A second asset to trust — **Medium, mitigation built**

`asset()` is pinned by the manager at epoch creation. An adapter built over a
pool that lists a different contract for "XLM" would silently hold the wrong
thing.

*Mitigation:* the constructor calls `get_reserve(asset)` on the pool before
storing anything, so a pool that does not list the asset fails the deployment
(`ReserveNotFound`) instead of failing at the first deposit.

*Open:* this only proves the pool knows the address. It does not prove the
address is the token the rest of Strata means by XLM.

### B7 — Testnet is not a market — **Low, documented**

The testnet pool is driven by a mock oracle and test actors. One epoch has now
been run through it (2026-11-05, 451 stroops of yield), and that proves the
plumbing and nothing about liquidity, losses or adversarial behaviour under real
conditions.

*Mitigation:* the design note says so, and this entry exists so a passing
testnet epoch cannot be read as evidence of anything else.

## State and operations

### R9 — Storage expiry — **Low, audited (persistent entries archive rather than vanish, but expire before max term without upkeep/restoration)**

An earlier version of this entry stated that an expired Soroban persistent entry
is *silently missing*. That is only true of `temporary()` storage (and of
Blend's user position in B4). Every entry in `epoch-manager` and `mock-vault`
is stored in `persistent()` storage: when a persistent entry's TTL reaches zero
it moves to the **archived** state in the ledger's BucketList / Hot Archive,
not to `None`. An archived entry cannot be read until it is restored (either
automatically by the transaction footprint in Protocol 23+ or explicitly with
`stellar contract restore`), which costs rent but **never destroys principal**.

#### Storage TTL inventory (`epoch-manager` and `mock-vault`)

| Contract | Entry / Key | Storage type | Written when | Extended when & by whom | `BUMP_THRESHOLD` | `BUMP_AMOUNT` |
| --- | --- | --- | --- | --- | --- | --- |
| `epoch-manager` | Contract instance & Wasm | Instance / Code | Deploy | Externally via `scripts/extend-ttl-testnet.sh` (admin/keeper) | — | `500_000` ledgers (~28.9 days) per script run |
| `epoch-manager` | `DataKey::Config` | Persistent | `__constructor` | `__constructor` only (`bump` in `lib.rs`); never re-written afterwards | `100_000` ledgers (~5.79 days) | `4_000_000` ledgers (~231.48 days) |
| `epoch-manager` | `DataKey::Epoch` | Persistent | `create_epoch`, `deposit`, `settle`, `claim`, `close_epoch` | Every lifecycle write (`store_epoch` -> `bump`) by whoever submits the tx | `100_000` ledgers (~5.79 days) | `4_000_000` ledgers (~231.48 days) |
| `epoch-manager` | `DataKey::Position(addr, tranche)` | Persistent | `deposit`, `claim` | `store_position` -> `bump` on the depositor's own `deposit` and `claim` only | `100_000` ledgers (~5.79 days) | `4_000_000` ledgers (~231.48 days) |
| `mock-vault` | Contract instance & Wasm | Instance / Code | Deploy | Externally via `scripts/extend-ttl-testnet.sh` | — | `500_000` ledgers (~28.9 days) per script run |
| `mock-vault` | `Admin`, `Asset`, `TotalAssets`, `TotalShares`, `RateBps`, `LastAccrualTs`, `ShareBalance(addr)` | Persistent | `__constructor`, `set_yield_rate`, `accrue`, `deposit`, `redeem` | **Never explicitly extended in contract code** (no `extend_ttl` calls in `mock-vault`); receives only the network's `min_persistent_entry_ttl` (`4_096` ledgers in SDK defaults, ~120 days on testnet) | None | None |

#### Wall-clock comparison (at measured 5 seconds / ledger)

| Quantity | Ledgers (at 5 s / ledger) | Wall-clock time |
| --- | --- | --- |
| `BUMP_THRESHOLD` (`epoch-manager`) | `100_000` | `500_000` s (**5.79 days**) |
| `BUMP_AMOUNT` (`epoch-manager`) | `4_000_000` | `20_000_000` s (**231.48 days**) |
| Soroban default `max_entry_ttl` (Protocol 23+) | `6_312_000` | `31_560_000` s (**365.28 days**) |
| Maximum epoch term (`MAX_TERM_SECONDS`) | `6_307_200` | `31_536_000` s (**365.00 days**) |
| Max epoch term + 30-day claim window | `6_825_600` | `34_128_000` s (**395.00 days**) |

#### Statement on whether a position can expire before it is claimed

**Yes.** At the measured 5-second testnet ledger cadence, `BUMP_AMOUNT = 4_000_000`
ledgers is **231.48 days**, whereas the maximum permitted epoch term is **365 days**
(`6_307_200` ledgers). A depositor who deposits at `t = 0` into an epoch whose
term exceeds `20_000_000` seconds (~231.48 days) — or who waits past day 231 to
claim — will have their `DataKey::Position(addr, tranche)` entry (as well as
`DataKey::Config`, which is only written in `__constructor`) reach `TTL = 0` and
archive **133.52 days before maturity**, and **163.52 days before the end of a
30-day post-maturity claim window**. In addition, because `extend_ttl` clamps to
the network's `max_entry_ttl` (~365.28 days), no single deposit-time bump can
cover a 365-day epoch plus a 30-day claim window (`6_825_600` ledgers) without
an intermediate renewal or restoration at claim time.

*Evidence:* pinned in `contracts/epoch-manager/src/test.rs` by:
- `storage_entries_record_expected_initial_ttls_across_manager_and_vault`
- `position_ttl_only_extends_once_remaining_life_reaches_bump_threshold`
- `position_archived_past_bump_amount_in_max_term_epoch_is_restored_and_claimable`

*How archival and restoration work in practice:* because `Position`, `Epoch`,
and `Config` are `persistent()` entries, archival does not lose the depositor's
principal. Under Protocol 23+ (Stellar CLI 28 / `soroban-sdk` 27), simulating
`claim` or restoring the entry via `stellar contract restore` brings the entry
back into live storage (see [`architecture.md`](architecture.md#storage-layout)).

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

### R20 — Per-transaction CPU, memory and ledger footprint limits — **Low, analysed**

Every state-changing entrypoint and read-only view on `epoch-manager` was
measured across `N = 1`, `10`, and `100` depositor positions using the
`CostEstimate` metering API of `soroban-sdk` 27.0.6 (`tests/src/resource_budget.rs`,
full tables in [`resource-report.md`](resource-report.md)).

*Findings:*
- **All entrypoints are strictly $O(1)$ on chain with respect to depositor count.**
  `DataKey::Epoch` tracks tranche principals and unclaimed totals as scalar
  `i128` counters, and each depositor's position lives in its own isolated
  `DataKey::Position(Address, Tranche)` key. No entrypoint iterates over
  depositors: ledger read/write entry counts, write bytes, and event bytes are
  constant across `1`, `10`, and `100` positions (`settle` uses `20` footprint
  entries and `1,628` write bytes on `MockVault`, and `37` footprint entries and
  `2,216` write bytes on `BlendAdapter`, regardless of `N`).
- **Well inside Stellar Mainnet limits.** The closest dimension to any network
  ceiling is the ledger footprint entry count on `deposit` (`42 / 400 = 10.50%`
  via `BlendAdapter`; `25 / 400 = 6.25%` via `MockVault`) and `settle`
  (`37 / 400 = 9.25%` via `BlendAdapter`; `20 / 400 = 5.00%` via `MockVault`).
  Contract event bytes peak at `1,172 / 16,384 B` (`7.15%`), write bytes peak at
  `2,564 / 132,096 B` (`1.94%`), memory peaks below `2.63%` of `40 MiB`, and CPU
  instructions peak below `0.79%` of `400M`.
- **Lifecycle-level note:** while `close_epoch` itself is $O(1)$ (`3` footprint
  entries), it requires `senior_unclaimed == 0 && junior_unclaimed == 0`, and
  `claim` requires `claimant.require_auth()` (one depositor per call). Closing
  an epoch with `N` positions therefore requires `N` separate `claim`
  transactions before `close_epoch` can succeed.

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

*Now also verified against a real network:* soroban-sdk 27.0.6 built under
Stellar CLI 28.1.0 and deployed to a protocol 29 testnet, where both contracts
settled real epochs. Note that the CLI ships `stellar-xdr` 28.0.0 against an SDK
27 workspace — that pairing was verified empirically rather than assumed, and a
future CLI bump could break it silently.

*Open:* no dependency review. Nothing checks that the CLI version used to build
is compatible with the SDK version pinned here.

*Audit, as it actually behaves:* the `dependency audit` job carries
`continue-on-error: true`, which keeps the *workflow* green, but the
`protect-main` ruleset requires that check by name, so a failing audit run does
block a pull request. Exactly one advisory is ignored, by ID, in
[`.cargo/audit.toml`](../.cargo/audit.toml), where the reasoning and the removal
condition are recorded:

- **RUSTSEC-2026-0387** — `crypto-bigint` 0.5.5, timing variability in
  operations intended to be constant-time, published 2026-10-23. Reached only
  through `soroban-env-host` → `k256`; `cargo tree -i crypto-bigint --target
  wasm32v1-none` finds nothing, so it is in neither deployed Wasm, and no
  contract here calls the secp256k1 host functions it backs. The fix needs
  `crypto-bigint >= 0.7.6`, which can only arrive through a `soroban-sdk`
  release, and 27.0.6 is the newest 27.x. Removing the ignore is part of the SDK
  bump this entry already says must be its own commit.

### R17 — Mainnet guardrail — **Low, closed**

This repository is testnet-only by policy. Policy is not a control.

*Now:* the control exists. `scripts/deploy-testnet.sh` holds an allowlist of
networks and refuses anything else **before** validating its other arguments, so
asking for `mainnet` fails on the network regardless of what else is wrong with
the invocation. `--self-test` exercises the refusal, and CI runs that on every
push. `run-epoch-demo.sh` and `extend-ttl-testnet.sh` carry the same guard.

*Open:* the allowlist also permits `local` and `futurenet`. Neither is mainnet
and neither has been used — only `testnet` was. Adding a real network is a
reviewed change to that list, not a flag.

### R17a — Unpinned, unsigned deployment — **Medium**

Nothing binds a deployed contract to a commit. `deployments/testnet.json` records
the wasm hashes and the SDK version, and the on-chain wasm hash was checked
against the local build, so a mismatch is *detectable* — but nothing verifies the
hash on mainnet, and there is no signature or attestation tying a running
contract to a reviewed commit.

*Open:* no reproducible or verifiable deployment story. A client cannot prove
which source produced a deployed contract. Worth fixing before anything holds
real value.

### R18 — Unaudited claims in the README — **Low**

The README must not imply the contracts have been reviewed.

*Mitigation:* the disclaimer is the first thing in the README and in every
document.

---

## Not yet written down

Honest list of risks I have not yet analysed, so they are absent above rather
than dismissed:

- Gas and resource limits on mainnet — **analysed in R20 and [resource-report.md](resource-report.md)** (`settle` and all other entrypoints are $O(1)$ per transaction across 1, 10 and 100 positions). Previously, while SDK v25+ enforced mainnet limits in tests, a worst-case
  `settle` with many depositors has not been measured.
- `V` manipulation at the settlement ledger for a vault with a real, movable
  price. The ordering half is analysed in R19; the market half needs a real
  pool and is tracked under B1 and B2.
- Formal verification of the `mul_div` rounding direction against the
  last-claim remainder, beyond the property tests.
- The economics of `MAX_SENIOR_RATIO_BPS` against realistic vault volatility.
  The constant is safe; whether 1.0x is *sufficient* is a modelling question
  nobody has answered.
