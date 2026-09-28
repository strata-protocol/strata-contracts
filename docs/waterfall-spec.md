# Strata Waterfall Spec

**Status:** source of truth. **Unaudited. Testnet only.**

This document defines the settlement math. Code that disagrees with this
document is a bug. Changing this document is a maintainer decision made in a
`spec` issue, never in a pull request (see
[CONTRIBUTING.md](../CONTRIBUTING.md#ground-rules)).

Everything here is integer arithmetic on `i128` base units. There are no
floating-point values anywhere in the settlement path, and none may be
introduced.

---

## 1. Purpose

Strata wraps any ERC-4626-style Soroban vault and splits the result of one
fixed-term epoch into two tranches:

- **Senior** — principal plus a fixed target rate, capped at that target,
  paid first.
- **Junior** — everything above the senior target, absorbing losses first.

The senior's claim is protected by the junior buffer. The buffer is only real
if the junior principal sits underneath the senior's target interest, which is
what the deposit gate in [§7](#7-deposit-gating) enforces.

---

## 2. Inputs

| Symbol | Type | Meaning | Domain |
| --- | --- | --- | --- |
| `S` | `i128` | Senior principal deposited into the epoch | `0 <= S <= MAX_PRINCIPAL` |
| `J` | `i128` | Junior principal deposited into the epoch | `0 <= J <= MAX_PRINCIPAL` |
| `r` | `u32` | Senior target rate, basis points per year | `0 <= r <= MAX_SENIOR_RATE_BPS` |
| `t` | `u64` | Epoch length in seconds | `0 < t <= MAX_EPOCH_TERM_SECONDS` |
| `V` | `i128` | Value redeemed from the underlying vault at maturity | `0 <= V` |

`V` is *not* known when the epoch opens. It is whatever the underlying vault
returns at settlement. `S` and `J` are fixed once deposits close.

All amounts are denominated in the underlying asset's base units. `r` and `t`
are the only things that are not amounts.

## 3. Constants

```
SECONDS_PER_YEAR            = 31_536_000          # 365 * 24 * 3600
BPS_DENOMINATOR              = 10_000              # 1.0 == 100%
SECONDS_PER_YEAR_DENOMINATOR = BPS_DENOMINATOR * SECONDS_PER_YEAR
                             = 315_360_000_000

MAX_SENIOR_RATE_BPS          = 10_000              # 100% APR
MAX_EPOCH_TERM_SECONDS       = 31_536_000          # 1 year
MAX_SENIOR_RATIO_BPS         = 10_000              # 1.0x, i.e. S <= J
MAX_PRINCIPAL                = 100_000_000_000_000_000_000_000_000  # 1e26
```

### Why these bounds

The bounds are not arbitrary; they are chosen so the settlement expression
cannot overflow and so the junior buffer provably covers the senior target.

**Overflow.** The only large intermediate product is `S * r * t`. With
`r <= 10_000` and `t <= 31_536_000`:

```
S * r * t  <=  S * 3.1536e11
1e26 * 3.1536e11 = 3.1536e37  <  i128::MAX = 1.7014e38
```

so `MAX_PRINCIPAL = 1e26` leaves roughly 5.4x headroom. `J * ratio` and
`S * 10_000` are both bounded by `1e30`, far inside `i128`. The implementation
uses checked arithmetic regardless, so a violation traps rather than wrapping.

**Buffer coverage.** Write `I = senior_interest = S * r * t / DENOM`. Because
`r <= 10_000` bps (100%/yr) and `t <= 31_536_000` s (1 year), the ratio
`r * t / SECONDS_PER_YEAR` is at most `1.0`, hence

```
I <= S
```

and because the gate in [§7](#7-deposit-gating) forces `S <= J` at every point
in the deposit window,

```
I <= S <= J
```

Two consequences, stated precisely:

- **The senior's target interest is never funded out of junior principal.** If
  the underlying earns exactly nothing (`V = S + J`), the senior is paid
  exactly `S + I` and the junior gets exactly `J` back. The junior is never
  required to make up a shortfall in the senior's target.
- **The senior is made whole against losses of at most `J - I`.** Writing
  `loss = S + J - V`, the senior receives its full `senior_due` if and only if
  `loss <= J - I`. So the real loss cushion is the buffer `J` *minus* the
  senior's target interest, and `I <= J` guarantees that cushion is
  non-negative.

Note the cushion is `J - I`, not `J`. The senior's own target interest eats
into the buffer rather than adding to it, so a buffer exactly equal to `I`
leaves a cushion of zero: the senior is made whole only in the break-even case.
This is a property of the [§5](#5-the-waterfall) ordering, not a defect, but
it is easy to assume otherwise and the assumption is wrong.

Raising `MAX_SENIOR_RATE_BPS` or `MAX_EPOCH_TERM_SECONDS` past the values above
invalidates `I <= S` and with it both conclusions.

---

## 4. Senior due

```
senior_due = S + (S * r * t) / (BPS_DENOMINATOR * SECONDS_PER_YEAR)
```

The division is **truncating** (rounds toward zero). The discarded sub-unit
fraction is never paid to the senior, so it stays in `V` and flows to the
junior. This is the only rounding in the settlement path, and it always
rounds in the junior's favour.

Worked values, `S = 1_000_000`, `r = 500` (5%/yr), `t = 31_536_000` (1 yr):

```
S * r * t = 1_000_000 * 500 * 31_536_000 = 1.5768e16
DENOM                                  = 3.1536e11
senior_interest                        = 1.5768e16 / 3.1536e11 = 50_000
senior_due                             = 1_000_000 + 50_000 = 1_050_000
```

## 5. The waterfall

```
if V >= senior_due:
    senior_payout = senior_due
    junior_payout = V - senior_due
else:
    senior_payout = V
    junior_payout = 0
```

Equivalently, and this is how it is written in code:

```
senior_payout = min(V, senior_due)
junior_payout = V - senior_payout
```

The senior is capped at its target. The junior absorbs any shortfall. The
junior is paid its principal back plus all yield above the senior target when
the epoch is healthy, and nothing at all when it is not.

**This is structurally exact.** Note that the split introduces no rounding of
its own: `senior_payout` is either `senior_due` or `V`, and `junior_payout` is
the remainder. The two always add back up to `V` by construction. Invariant 1
below therefore holds by construction and not merely by testing.

### Loss case

`V < senior_due` means the underlying lost money, or earned less than the
senior target. The senior takes the loss, but only down to the floor of zero —
a senior depositor can lose their entire principal. The junior is wiped out
first by construction: the junior is junior precisely because its principal
sits below the senior's claim.

---

## 6. Invariants

Each invariant has at least one property test in `tests/waterfall_props.rs`.
Treat a failure of any of them as a critical bug.

### 1. Conservation

```
senior_payout + junior_payout == V
```

No value is created and no value is lost. The rounding remainder from
[§4](#4-senior-due) accrues to the junior, never to the senior.

### 2. Senior cap

```
senior_payout <= senior_due
```

The senior never receives more than its target, even in a very good epoch.

### 3. Senior paid first

```
junior_payout > 0  implies  senior_payout == senior_due
```

If the junior is paid anything at all, the senior was made whole. The converse
is **not** required and is false: when `V == senior_due` exactly, the senior is
made whole and the junior receives zero.

### 4. Monotonicity in `V`

For fixed `S`, `r`, `t`, both payouts are non-decreasing as `V` increases.

```
V1 <= V2  implies  senior_payout(V1) <= senior_payout(V2)
V1 <= V2  implies  junior_payout(V1) <= junior_payout(V2)
```

Both are `min(V, senior_due)` and `V - min(V, senior_due)` respectively, which
are continuous, piecewise-linear, and monotone. They are **non**-decreasing,
not strictly increasing: `senior_payout` is flat for all `V >= senior_due`, and
`junior_payout` is flat for all `V <= senior_due`. This matters economically —
a depositor is never better off if the underlying does worse.

### 5. No overflow

For all inputs inside the documented limits in [§3](#3-constants), the
settlement expression neither overflows nor panics, and both payouts are
non-negative.

---

## 7. Deposit gating

Let `S_total` be total senior principal deposited into the epoch so far and
`J_total` total junior principal, both *after* the pending deposit. A senior
deposit of `d` is accepted only if:

```
J_total > 0                            # see the deviation note below
S_total * BPS_DENOMINATOR <= J_total * max_senior_ratio_bps
```

with `0 < max_senior_ratio_bps <= MAX_SENIOR_RATIO_BPS` fixed at epoch
creation.

The second condition is written cross-multiplied rather than as
`S_total <= J_total * ratio / 10_000` on purpose: dividing would silently floor
the cap and make the effective ratio depend on the size of `J`. Multiplying
out keeps the comparison exact.

Junior deposits are never gated. Junior capital is what makes the structure
safe, so it is always welcome.

### Deviation note: the `J_total > 0` condition

The first condition is an approved refinement of the original rule, which read
only `S <= J * max_senior_ratio`.

That rule is **vacuously true on an empty epoch**. With `S_total = 0` and
`J_total = 0` it evaluates to `0 <= 0`, so an arbitrarily large first senior
deposit would be accepted with no junior capital whatsoever. That directly
contradicts the stated intent that the junior buffer is always real, and
[§3](#why-these-bounds) shows the buffer-coverage property fails at `S = 0,
J = 0`.

The extra condition closes the hole in one line and changes no payout
arithmetic. Approved 2026-09-28 as a `spec` issue. If you want the literal
original behaviour, this is the line to remove — and the corresponding property
test in `tests/waterfall_props.rs`.

---

## 8. Distribution to depositors

A tranche's payout is split pro-rata by principal. For a depositor holding
`S_user` of the `S_total` senior principal:

```
claim_senior = senior_payout * S_user / S_total     # truncating
```

and symmetrically for the junior. Ordinary per-depositor rounding leaves dust:
at most `n - 1` base units for `n` depositors.

**The final claim in a tranche absorbs the remainder.** When the last
outstanding share in a tranche is claimed, that claim is paid
`total_payout - already_paid_out` rather than the pro-rata floor, so the
tranche pays out exactly `senior_payout` and `junior_payout` in total.

This keeps invariant 1 exact at the depositor level: the sum of all senior
claims is exactly `senior_payout`, and the sum of all junior claims is exactly
`junior_payout`, so the sum of all claims is exactly `V`.

It is not griefable. The remainder is only ever available to a depositor who
already holds a claimable position in that tranche, and the amount is bounded
by the dust above.

---

## 9. Worked examples

Assume `r = 500` bps/yr, `t = 31_536_000` s, so `I = 50_000` and
`senior_due = 1_050_000`. Total principal is `2_000_000`, and the loss cushion
is `J - I = 950_000`: below that loss the senior is made whole in full.

| Case | `V` | `loss` | `senior_payout` | `junior_payout` | Senior whole? |
| --- | --- | --- | --- | --- | --- |
| Healthy | 2_200_000 | -200_000 (gain) | 1_050_000 | 1_150_000 | yes |
| Exact hit | 1_050_000 | 950_000 | 1_050_000 | 0 | yes, at the cushion limit |
| Buffer absorbs | 1_100_000 | 900_000 | 1_050_000 | 50_000 | yes |
| Just past cushion | 1_049_999 | 950_001 | 1_049_999 | 0 | no, by 1 unit |
| Cushion gone | 900_000 | 1_100_000 | 900_000 | 0 | no, loses 150_000 |
| Wipeout | 0 | 2_000_000 | 0 | 0 | no |

All rows are `S = J = 1_000_000`, and every row satisfies
`senior_payout + junior_payout == V`.

The "Buffer absorbs" row is the case the structure exists for: a loss of
900_000, which is under the 950_000 cushion, leaves the senior untouched and
costs the junior 950_000 of its 1_000_000.

The "Cushion gone" row shows the limit of that protection. The loss of
1_100_000 exceeds the cushion, so the junior absorbs its full 1_000_000
principal and the senior is left covering the remaining 150_000. A senior
depositor can lose their entire principal: `senior_payout` floors at `V`, and
`V` floors at zero.

---

## 10. Out of scope

Deliberately not specified here, and not implemented:

- Variable or floating senior rates.
- Multiple concurrent epochs (there is exactly one active epoch; see
  [architecture.md](architecture.md)).
- Recurring or rolling epochs, and auto-rollover at maturity.
- Fees of any kind, performance or otherwise.
- Early exit, secondary trading, or transfer of tranche positions.
- Re-opening or re-parameterising a settled epoch.
- Recovery paths if the underlying vault is broken or the epoch is unsettled
  at the end of its term. See [risks.md](risks.md).
