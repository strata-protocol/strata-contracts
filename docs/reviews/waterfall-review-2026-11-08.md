# Independent review of `contracts/waterfall` against `docs/waterfall-spec.md`

- **Date:** `2026-11-08`
- **Scope:** [`contracts/waterfall/src/lib.rs`](../../contracts/waterfall/src/lib.rs) (332 lines), [`contracts/waterfall/src/tests.rs`](../../contracts/waterfall/src/tests.rs) (321 lines), [`tests/src/waterfall_props.rs`](../../tests/src/waterfall_props.rs) (454 lines), and the error-mapping / caller boundary in [`contracts/epoch-manager/src/lib.rs`](../../contracts/epoch-manager/src/lib.rs), checked against [`docs/waterfall-spec.md`](../waterfall-spec.md) (363 lines) at commit `dcdab58319994e4dc18b7862a162c74ff5f27b95`.
- **Closes:** issue #6. Per the issue ground rules, this PR modifies neither `contracts/waterfall` nor `docs/waterfall-spec.md`.

---

## 1. Executive summary

Within `contracts/waterfall/src/lib.rs` itself, the core settlement arithmetic (`senior_due`, `settle`, `check_senior_deposit`, `max_senior_total`) faithfully implements `docs/waterfall-spec.md` §§2–7 in checked `i128` integer arithmetic, and all five spec invariants hold across the crate's documented input domain (`0 <= S, J <= 10^26`, `0 <= r <= 10_000`, `1 <= t <= 31_536_000`, `0 <= V`).

However, this independent review uncovered **five concrete findings** — including **two High-severity funds-locking defects** at the boundary between `docs/waterfall-spec.md` / `strata-waterfall` and `epoch-manager`:

| ID | Title | Severity | Category / Label | Location |
| --- | --- | --- | --- | --- |
| **F1** | `docs/waterfall-spec.md` §8 pro-rata product `senior_payout * S_user` overflows `i128` at $1.304 \times 10^{19}$ ($7.66 \times 10^6\times$ below `MAX_PRINCIPAL = 10^26`), permanently bricking multi-depositor `claim()` | **High** | `spec` / `bug` | `docs/waterfall-spec.md` §§3, 8; `contracts/epoch-manager/src/lib.rs:550, 835` |
| **F2** | `EpochManager::deposit` validates `amount <= MAX_PRINCIPAL` per call but never validates post-deposit `epoch.junior_total <= MAX_PRINCIPAL`, allowing cumulative Junior deposits $> 10^{26}$ that permanently brick `settle()` with `WaterfallError::PrincipalTooLarge` | **High** | `bug` | `contracts/epoch-manager/src/lib.rs:346-351`; `contracts/waterfall/src/lib.rs:179` |
| **F3** | `pub fn senior_gate_holds` omits input validation and uses unchecked `*`, panicking on `i128` overflow and returning `true` for negative `senior_total` (e.g. `senior_gate_holds(-1, 0, 10_000) == true`) | **Low** | `bug` | `contracts/waterfall/src/lib.rs:256-264` |
| **F4** | Property test gaps in `tests/src/waterfall_props.rs`: `invariant_2_is_saturated_for_any_healthy_epoch` tests `small_value()` without asserting saturation (`senior_payout == senior_due`), `value()` caps `V` at $10^{29} \ll \texttt{i128::MAX}$, and `delta()` rarely samples `d == 0` at `V == senior_due` | **Low** | `test` | `tests/src/waterfall_props.rs:97-118, 197-208, 236-252` |
| **F5** | Doc/code mismatches in `strata_waterfall::settle` (claims `junior_principal` is checked for the buffer invariant `S <= J`, which it is not) and `epoch_manager::map_waterfall` (claims 1-to-1 variant mapping while collapsing 5 variants into `Error::InvalidParameter`, including `WaterfallError::Overflow` instead of `Error::ArithmeticOverflow`) | **Low** | `docs` / `bug` | `contracts/waterfall/src/lib.rs:165-166`; `contracts/epoch-manager/src/lib.rs:871-885` |

---

## 2. Verification of the five spec invariants

### Invariant 1 — Conservation (`senior_payout + junior_payout == V`, spec §6.1)

- **Code check (`contracts/waterfall/src/lib.rs:181-196`):**
  ```rust
  if value_redeemed < 0 {
      return Err(WaterfallError::NegativeAmount);
  }
  let due = senior_due(senior_principal, rate_bps, term_seconds)?;
  let senior_payout = if value_redeemed >= due {
      due
  } else {
      value_redeemed
  };
  let junior_payout = value_redeemed - senior_payout;
  ```
  Because `validate_common` ensures `senior_principal >= 0` and `senior_due >= senior_principal >= 0`, and line 181 ensures `value_redeemed >= 0`, both branches satisfy `0 <= senior_payout <= value_redeemed <= i128::MAX`. Therefore `value_redeemed - senior_payout` cannot underflow or overflow `i128`, and `senior_payout + junior_payout == value_redeemed` holds by algebraic identity.
- **Property tests (`tests/src/waterfall_props.rs:141-178`):**
  - `invariant_1_payouts_sum_to_value` (lines 142-151)
  - `invariant_1_holds_near_the_cap_boundary` (lines 154-169)
  - `invariant_1_survives_interest_truncation` (lines 172-178)
- **Would the property tests catch a violation?**
  - **Yes** for any change to `settle` that computes `junior_payout` independently of `V - senior_payout` or loses rounding dust across `0 <= V <= 10^29`.
  - **Gaps:**
    1. `value()` (`waterfall_props.rs:97-105`) caps `V` at `MAX_PRINCIPAL.saturating_mul(1_000) = 10^29`, so `V in (10^29, i128::MAX]` is never tested (see **F4**).
    2. These tests only check tranche-level conservation in `strata_waterfall::settle`; depositor-level conservation (`sum(claims) == V` under spec §8) lives in `contracts/epoch-manager` and fails for large principals due to **F1**.

### Invariant 2 — Senior cap (`senior_payout <= senior_due`, spec §6.2)

- **Code check (`contracts/waterfall/src/lib.rs:189-193`):**
  In the first branch (`value_redeemed >= due`), `senior_payout == due`. In the second branch (`value_redeemed < due`), `senior_payout == value_redeemed < due`. Thus `senior_payout <= senior_due` holds unconditionally.
- **Property tests (`tests/src/waterfall_props.rs:185-208`):**
  - `invariant_2_senior_never_exceeds_its_due` (lines 185-194)
  - `invariant_2_is_saturated_for_any_healthy_epoch` (lines 197-208)
- **Would the property tests catch a violation?**
  - **Yes** for an *overpayment* (`senior_payout > senior_due`).
  - **No (in `invariant_2_*` alone) for an *underpayment* on a healthy epoch (`senior_payout < senior_due` when `V >= senior_due`):** Despite being named `invariant_2_is_saturated_for_any_healthy_epoch`, lines 197-208 draw `v in small_value()` (`0..=1_000_000`, which is almost always a *loss* epoch when `s` is drawn from `principal()` up to `10^26`) and only assert `g.senior_payout <= g.senior_due` and `g.senior_payout <= v`. It never asserts `if v >= g.senior_due { prop_assert_eq!(g.senior_payout, g.senior_due); }` (see **F4**). A mutation that returned `senior_payout = 0, junior_payout = V` would pass both `invariant_1_*` and `invariant_2_*` tests and would only be caught by `invariant_3_*` when `V > 0`.

### Invariant 3 — Senior paid first (`junior_payout > 0 implies senior_payout == senior_due`, spec §6.3)

- **Code check (`contracts/waterfall/src/lib.rs:189-195`):**
  When `value_redeemed < due`, `senior_payout = value_redeemed` and `junior_payout = value_redeemed - value_redeemed = 0`. Contrapositively, `junior_payout > 0` implies `value_redeemed >= due`, where `senior_payout = due`. As noted in spec §6.3, the converse is false at the single point `V == senior_due`, where `senior_payout == senior_due` and `junior_payout == 0`.
- **Property tests (`tests/src/waterfall_props.rs:219-252`):**
  - `invariant_3_junior_is_paid_only_after_the_senior_is_whole` (lines 219-234)
  - `invariant_3_holds_at_the_boundary` (lines 237-252)
- **Would the property tests catch a violation?**
  - **Yes** whenever `junior_payout > 0` while `senior_payout != senior_due`.
  - **Boundary sampling note:** In `invariant_3_holds_at_the_boundary` (and `invariant_1_holds_near_the_cap_boundary`), `d` is drawn uniformly from `delta()` (`-1_000_000..=1_000_000`), and the test checks `[due + d - 1, due + d, due + d + 1]`. Because `d == 0` has probability `1 / 2,000,001` per case, a 256-case `proptest` run has only a `0.013%` chance of testing the exact boundary triple `[due - 1, due, due + 1]` (see **F4**). An off-by-one mutation (`value_redeemed > due` instead of `value_redeemed >= due`) is actually identical mathematically because at `V == due` both branches evaluate to `due`; however, an off-by-one mutation `if value_redeemed >= due - 1 { due }` would pay `senior_payout = due` and `junior_payout = -1` at `V = due - 1`, which is caught by `the_cushion_boundary_is_sharp` in `contracts/waterfall/src/tests.rs:86` rather than by the 256-case random `delta()` draw.

### Invariant 4 — Monotonicity in `V` (`V1 <= V2 implies payout(V1) <= payout(V2)`, spec §6.4)

- **Code check (`contracts/waterfall/src/lib.rs:189-195`):**
  For fixed `(S, r, t)`, `due` is constant. `senior_payout(V) = min(V, due)` has slope `1` on `[0, due]` and slope `0` on `[due, ∞)`. `junior_payout(V) = V - min(V, due) = max(0, V - due)` has slope `0` on `[0, due]` and slope `1` on `[due, ∞)`. Both are continuous and non-decreasing everywhere on `V >= 0`.
- **Property tests (`tests/src/waterfall_props.rs:261-316`):**
  - `invariant_4_both_payouts_are_monotone_in_v` (lines 261-283)
  - `invariant_4_is_flat_above_the_cap_and_below_it` (lines 286-316)
- **Would the property tests catch a violation?**
  - **Yes** for general non-monotonicity, with one sampling limitation: `invariant_4_both_payouts_are_monotone_in_v` picks `v1 in value()` (up to $10^{29}$) and `extra in 0..=10^9`, so `[v1, v2]` rarely straddles `senior_due` when `S > 10^{12}`, and `invariant_4_is_flat_above_the_cap_and_below_it` tests `[due - 1_000_000, due]` and `[due + 1_000_000, due + 2_000_000]`, skipping `(due, due + 1_000_000)`.

### Invariant 5 — No overflow within documented limits (spec §6.5)

- **Code check (`contracts/waterfall/src/lib.rs:123-202`):**
  `validate_common` and `validate_principal` reject any input outside `0 <= S, J <= MAX_PRINCIPAL`, `0 <= r <= MAX_SENIOR_RATE_BPS`, `1 <= t <= MAX_EPOCH_TERM_SECONDS`, and `0 <= V`. All arithmetic in `senior_due` and `settle` uses `checked_mul`, `checked_div`, and `checked_add`.
- **Property tests (`tests/src/waterfall_props.rs:326-372`):**
  - `invariant_5_the_documented_corner_settles_without_overflow` (lines 326-339)
  - `invariant_5_never_panics_at_the_extreme_corner` (lines 342-353)
  - `invariant_5_senior_due_never_exceeds_twice_the_principal` (lines 356-372)
- **Would the property tests catch a violation?**
  - **Yes** for `senior_due` and `settle` across `S, J <= MAX_PRINCIPAL`.
  - **No** for `pub fn senior_gate_holds` when called directly outside `check_senior_deposit` (**F3**), and **no** for the spec §8 depositor pro-rata multiplication `senior_payout * S_user / S_total` (**F1**).

---

## 3. Rounding rule, `senior_due` boundary, and error mapping

### A. Rounding rule (`docs/waterfall-spec.md` §4 & §7)

1. **Interest truncation (`senior_due`, `contracts/waterfall/src/lib.rs:136-148`):**
   ```rust
   let numerator = senior_principal
       .checked_mul(r)
       .and_then(|p| p.checked_mul(t))
       .ok_or(WaterfallError::Overflow)?;
   let interest = numerator
       .checked_div(SECONDS_PER_YEAR_DENOMINATOR as i128)
       .ok_or(WaterfallError::Overflow)?;
   ```
   Because `validate_common` enforces `senior_principal >= 0`, `r >= 0`, and `t >= 1`, `numerator >= 0`. Dividing a non-negative `i128` by `SECONDS_PER_YEAR_DENOMINATOR = 315_360_000_000 > 0` via Rust's truncating integer division (`checked_div`) is identical to mathematical floor division:
   $$I = \left\lfloor \frac{S \cdot r \cdot t}{315{,}360{,}000{,}000} \right\rfloor$$
   Any fractional stroop $\epsilon = \frac{(S \cdot r \cdot t) \bmod 315{,}360{,}000{,}000}{315{,}360{,}000{,}000} \in [0, 1)$ is discarded from `senior_due`. In `settle`, whenever `V >= senior_due`, `junior_payout = V - senior_due = V - S - I`, so the uncollected fraction $\epsilon$ stays in `junior_payout`. The senior is never overpaid by rounding.
2. **Deposit gate and room helper (`check_senior_deposit`, `senior_gate_holds`, `max_senior_total`, `contracts/waterfall/src/lib.rs:222-279`):**
   - `senior_gate_holds` checks `S * 10_000 <= J * ratio_bps` without division.
   - `max_senior_total` computes `room = (J * ratio_bps) / 10_000`. Because `J >= 0` and `ratio_bps >= 1`, integer division floors: $\text{room} = \lfloor J \cdot \text{ratio\_bps} / 10{,}000 \rfloor$. Since $S$ is an integer, $S \cdot 10{,}000 \le J \cdot \text{ratio\_bps} \iff S \le \lfloor J \cdot \text{ratio\_bps} / 10{,}000 \rfloor = \text{room}$. Thus the cross-multiplied gate and the floored `max_senior_total` helper agree on every in-range integer input (pinned by `gate_admits_exactly_the_configured_room` in `waterfall_props.rs:377`).

### B. Behaviour at the `senior_due` boundary

Let $D = \text{senior\_due}(S, r, t) = S + I \ge 0$. Tracing `settle(S, J, r, t, V)` (`contracts/waterfall/src/lib.rs:189-201`) across the three points straddling $D$ (for $D \ge 1$):

| `V` | Branch taken (`value_redeemed >= due`) | `senior_payout` | `junior_payout` (`V - senior_payout`) | Sum (`senior + junior`) | Economic meaning |
| --- | --- | --- | --- | --- | --- |
| $D - 1$ | `false` (`else`) | $D - 1$ | $(D - 1) - (D - 1) = 0$ | $D - 1 = V$ | Loss exceeds cushion $J - I$ by 1 unit; junior is wiped out (`0`), senior loses 1 unit below target |
| $D$ | `true` (`if`) | $D$ | $D - D = 0$ | $D = V$ | Exact cushion limit ($\text{loss} = J - I$); senior is made whole (`D`), junior receives `0` |
| $D + 1$ | `true` (`if`) | $D$ | $(D + 1) - D = 1$ | $D + 1 = V$ | Senior is capped at `D`; first unit above `D` flows to junior (`1`) |

At $D = 0$ (which occurs if and only if $S = 0$), every $V \ge 0$ satisfies `V >= 0 == due`, yielding `senior_payout = 0` and `junior_payout = V`.

### C. Error mapping (`WaterfallError` $\to$ `epoch_manager::Error`)

Comparing `WaterfallError` (`contracts/waterfall/src/lib.rs:76-97`) with `map_waterfall` (`contracts/epoch-manager/src/lib.rs:875-885`):

| `WaterfallError` variant | Trigger condition in `strata-waterfall` | Mapped `epoch_manager::Error` (`map_waterfall`) | Discriminant | Reviewer assessment |
| --- | --- | --- | ---: | --- |
| `NegativeAmount` | `principal < 0` or `value_redeemed < 0` | `Error::InvalidAmount` | `8` | Exact match |
| `PrincipalTooLarge` | `principal > MAX_PRINCIPAL` (`10^26`) | `Error::InvalidParameter` | `9` | Collapses into generic `InvalidParameter`; see **F2** and **F5** |
| `RateTooHigh` | `rate_bps > MAX_SENIOR_RATE_BPS` (`10_000`) | `Error::InvalidParameter` | `9` | Collapses into generic `InvalidParameter` |
| `TermOutOfRange` | `term_seconds == 0` or `> MAX_EPOCH_TERM_SECONDS` (`31_536_000`) | `Error::InvalidParameter` | `9` | Collapses into generic `InvalidParameter` |
| `RatioOutOfRange` | `max_senior_ratio_bps == 0` or `> MAX_SENIOR_RATIO_BPS` (`10_000`) | `Error::InvalidParameter` | `9` | Collapses into generic `InvalidParameter` |
| `Overflow` | Arithmetic overflow in `senior_due` | `Error::InvalidParameter` | `9` | **Mismatch (F5):** `epoch-manager` defines `Error::ArithmeticOverflow = 12`, but `map_waterfall` maps `WaterfallError::Overflow` to `Error::InvalidParameter = 9` |
| `SeniorGateViolated` | `junior_total <= 0` or `S * 10_000 > J * ratio_bps` | `Error::SeniorGateViolated` | `10` | Exact match |

---

## 4. Independent recomputation of overflow limits (`i128`)

Signed 128-bit integer bounds:
$$\texttt{i128::MAX} = 2^{127} - 1 = 170{,}141{,}183{,}460{,}469{,}231{,}731{,}687{,}303{,}715{,}884{,}105{,}727 \approx 1.7014118346 \times 10^{38}$$

Documented constants (`docs/waterfall-spec.md` §3, `contracts/waterfall/src/lib.rs:27-58`):
- `SECONDS_PER_YEAR = 365 * 24 * 3600 = 31,536,000`
- `BPS_DENOMINATOR = 10,000`
- `SECONDS_PER_YEAR_DENOMINATOR = 10,000 * 31,536,000 = 315,360,000,000` ($3.1536 \times 10^{11}$, fits in `u64::MAX` $= 1.84467 \times 10^{19}$)
- `MAX_SENIOR_RATE_BPS = 10,000`
- `MAX_EPOCH_TERM_SECONDS = 31,536,000`
- `MAX_SENIOR_RATIO_BPS = 10,000`
- `MAX_PRINCIPAL = 100,000,000,000,000,000,000,000,000` ($10^{26}$)

### A. Operations inside `contracts/waterfall/src/lib.rs` (Spec §§3–7) — **HOLD for `i128`**

| Expression | Location | Maximum value within documented bounds | Headroom vs. `i128::MAX` ($1.7014 \times 10^{38}$) | Holds for `i128`? |
| --- | --- | ---: | ---: | --- |
| `senior_principal * r` | `lib.rs:136-137` | $10^{26} \times 10^4 = 10^{30}$ | $1.7014 \times 10^8\times$ | **Yes** |
| `(senior_principal * r) * t` | `lib.rs:138` | $10^{30} \times 31{,}536{,}000 = 3.1536 \times 10^{37}$ | $\frac{2^{127}-1}{3.1536 \times 10^{37}} \approx \mathbf{5.39514\times}$ | **Yes** (matches spec's "roughly 5.4x headroom") |
| `interest = numerator / DENOM` | `lib.rs:141-143` | $\frac{3.1536 \times 10^{37}}{3.1536 \times 10^{11}} = 10^{26}$ | $1.7014 \times 10^{12}\times$ | **Yes** |
| `senior_due = S + interest` | `lib.rs:145-147` | $10^{26} + 10^{26} = 2 \times 10^{26}$ | $8.5070 \times 10^{11}\times$ | **Yes** |
| `junior_payout = V - senior_payout` | `lib.rs:195` | $V \le \texttt{i128::MAX}$, $0 \le \text{senior\_payout} \le V$ | Exact (no overflow/underflow for any $V \ge 0$) | **Yes** |
| `senior_total * 10_000` | `lib.rs:261` | $10^{26} \times 10^4 = 10^{30}$ (when validated) | $1.7014 \times 10^8\times$ | **Yes** when validated via `check_senior_deposit`; **No** if `senior_gate_holds` is called directly with unvalidated `i128` (**F3**) |
| `junior_total * max_senior_ratio_bps` | `lib.rs:262, 276` | $10^{26} \times 10^4 = 10^{30}$ (when validated) | $1.7014 \times 10^8\times$ | **Yes** when validated |

The exact maximum principal $S_{\max}$ before `S * r * t` overflows `i128` at $r = 10{,}000$ and $t = 31{,}536{,}000$ is:
$$S_{\max} = \left\lfloor \frac{2^{127} - 1}{315{,}360{,}000{,}000} \right\rfloor = 539{,}514{,}153{,}540{,}300{,}709{,}448{,}526{,}457 \approx 5.39514 \times 10^{26}$$

### B. Operation in `docs/waterfall-spec.md` §8 ("Distribution to depositors") — **DOES NOT HOLD for `i128` at `MAX_PRINCIPAL = 10^26` (Finding F1)**

`docs/waterfall-spec.md` §8 (lines 299-301) specifies the per-depositor pro-rata claim formula:
$$\text{claim\_senior} = \lfloor \text{senior\_payout} \times S_{\text{user}} / S_{\text{total}} \rfloor, \quad \text{claim\_junior} = \lfloor \text{junior\_payout} \times J_{\text{user}} / J_{\text{total}} \rfloor$$
which `contracts/epoch-manager/src/lib.rs:550, 725, 835-842, 868` implements in `i128` via `mul_div`:
```rust
fn mul_div(a: i128, b: i128, c: i128) -> Result<i128, Error> {
    if c <= 0 {
        return Err(Error::ArithmeticOverflow);
    }
    a.checked_mul(b)
        .map(|p| p / c)
        .ok_or(Error::ArithmeticOverflow)
}
```
Here `a = tranche_payout` and `b = my_principal` are **both** $O(\texttt{MAX\_PRINCIPAL})$ quantities:
- At the documented limit $S_{\text{total}} = 10^{26}$ (e.g. two senior depositors with $S_1 = 5 \times 10^{25}$ and $S_2 = 5 \times 10^{25}$) and $\text{senior\_payout} = 2 \times 10^{26}$:
  $$\text{senior\_payout} \times S_1 = (2 \times 10^{26}) \times (5 \times 10^{25}) = 10^{52} \gg \texttt{i128::MAX} \approx 1.7014 \times 10^{38}$$
  The product exceeds `i128::MAX` by a factor of **$5.88 \times 10^{13}\times$**!
- Even for two equal depositors ($S_1 = S_2 = S_{\text{user}}$, $S_{\text{total}} = 2 S_{\text{user}}$) with 0% interest ($\text{senior\_payout} = 2 S_{\text{user}}$), `mul_div` overflows `i128` as soon as:
  $$2 S_{\text{user}}^2 > 2^{127} - 1 \implies S_{\text{user}} > \sqrt{\frac{2^{127} - 1}{2}} = 9{,}223{,}372{,}036{,}854{,}775{,}807 = \texttt{i64::MAX} \approx 9.223 \times 10^{18}$$
- Thus `MAX_PRINCIPAL = 10^26` is **10.8 million times larger** than the maximum principal ($9.223 \times 10^{18}$) that can safely execute `docs/waterfall-spec.md` §8's `tranche_payout * user_principal / tranche_total` in `i128` without widening `mul_div` to 256-bit arithmetic!

---

## 5. Detailed findings and reproductions

### Finding F1 — **[High, `spec` / `bug`]** Spec §8 pro-rata multiplication `tranche_payout * user_principal` overflows `i128` for multi-depositor tranches above $\sim 9.22 \times 10^{18}$, permanently locking funds in `claim()`

- **Labels:** `spec`, `security`
- **Where:** `docs/waterfall-spec.md` §3 (lines 66-76) & §8 (lines 296-302); `contracts/epoch-manager/src/lib.rs:550, 725, 835-842, 868`.
- **Description:** `docs/waterfall-spec.md` §3 derives `MAX_PRINCIPAL = 10^26` solely from `S * r * t <= 3.1536e37 < i128::MAX`. It omits the intermediate product in §8 (`senior_payout * S_user / S_total` and `junior_payout * J_user / J_total`). When a tranche has $\ge 2$ depositors, every claim except the last one in the tranche takes the `remaining != 0` branch in `EpochManager::claim` (`contracts/epoch-manager/src/lib.rs:550`) and calls `mul_div(tranche_payout, my_principal, tranche_total)`, which computes `tranche_payout.checked_mul(my_principal)` in `i128`. Whenever `tranche_payout * my_principal > i128::MAX` (`1.7014e38`), `claim()` reverts with `Error::ArithmeticOverflow (12)` for **every** depositor in the tranche (since whichever depositor attempts to claim first has `remaining > 0`), permanently locking the entire tranche payout in `epoch-manager` and preventing `close_epoch` from ever succeeding.
- **Worked numbers:**
  - `J = 20_000_000_000_000_000_000` ($2 \times 10^{19} \ll \texttt{MAX\_PRINCIPAL} = 10^{26}$), split between `junior_1 = 10^{19}` and `junior_2 = 10^{19}`.
  - `S = 0`, `r = 500`, `t = 31_536_000`, `V = 20_000_000_000_000_000_000` ($2 \times 10^{19}$).
  - `settle()` succeeds with `junior_payout = 20_000_000_000_000_000_000`.
  - When `junior_1` (or `junior_2`) calls `claim`, `remaining = 10^{19} != 0`, and `mul_div` computes:
    $$\text{junior\_payout} \times \text{my\_principal} = (2 \times 10^{19}) \times 10^{19} = 2 \times 10^{38} > \texttt{i128::MAX} = 1.70141183 \times 10^{38}$$
  - Both `claim(junior_1, Junior)` and `claim(junior_2, Junior)` fail with `Err(Error::ArithmeticOverflow)`.
- **Reproduction test:**
  ```rust
  #[test]
  fn f1_multi_depositor_claim_overflows_i128_far_below_max_principal() {
      let env = Env::default();
      env.mock_all_auths();
      let admin = Address::generate(&env);
      let asset = env.register_stellar_asset_contract_v2(admin.clone()).address();
      let vault = env.register(MockVault, (admin.clone(), asset.clone()));
      let manager = env.register(EpochManager, (admin.clone(), vault.clone()));
      let mgr = EpochManagerClient::new(&env, &manager);

      // 1e19 is 10,000,000x below MAX_PRINCIPAL (1e26) and below MockVault::MAX_ASSETS (1e24).
      let deposit_each: i128 = 10_000_000_000_000_000_000; // 1e19
      let j1 = Address::generate(&env);
      let j2 = Address::generate(&env);
      StellarAssetClient::new(&env, &asset).mint(&j1, &deposit_each);
      StellarAssetClient::new(&env, &asset).mint(&j2, &deposit_each);

      mgr.create_epoch(&31_536_000, &500, &10_000);
      mgr.deposit(&j1, &Tranche::Junior, &deposit_each);
      mgr.deposit(&j2, &Tranche::Junior, &deposit_each);

      env.ledger().set_timestamp(env.ledger().timestamp() + 31_536_000);
      mgr.settle();

      // Neither j1 nor j2 can claim: 2e19 * 1e19 = 2e38 > i128::MAX (1.7014e38).
      assert_eq!(
          mgr.try_claim(&j1, &Tranche::Junior),
          Err(Ok(Error::ArithmeticOverflow))
      );
      assert_eq!(
          mgr.try_claim(&j2, &Tranche::Junior),
          Err(Ok(Error::ArithmeticOverflow))
      );
  }
  ```
- **Remediation options:** Either widen `mul_div` to a 256-bit intermediate product (so `a * b / c` never overflows for `a, b <= 2 * MAX_PRINCIPAL`) or lower `MAX_PRINCIPAL` in `docs/waterfall-spec.md` §3 to account for §8.

---

### Finding F2 — **[High, `bug`]** `EpochManager::deposit` never validates post-deposit `epoch.junior_total <= MAX_PRINCIPAL`, allowing cumulative Junior deposits $> 10^{26}$ that permanently brick `settle()`

- **Labels:** `bug`, `security`
- **Where:** `contracts/epoch-manager/src/lib.rs:326, 346-351` and `contracts/waterfall/src/lib.rs:179`.
- **Description:** In `EpochManager::deposit`:
  - Line 326 calls `check_amount(amount)?`, which calls `strata_waterfall::validate_principal(amount)` on the **single deposit** (`amount <= MAX_PRINCIPAL`).
  - For `Tranche::Senior` (lines 333-343), `check_senior_deposit(new_senior, epoch.junior_total, ...)` validates `new_senior <= MAX_PRINCIPAL`.
  - For `Tranche::Junior` (lines 346-351):
    ```rust
    Tranche::Junior => {
        epoch.junior_total = epoch
            .junior_total
            .checked_add(amount)
            .ok_or(Error::ArithmeticOverflow)?;
    }
    ```
    `validate_principal(epoch.junior_total)` is **never called**! Two Junior deposits of $6 \times 10^{25}$ (each $\le \texttt{MAX\_PRINCIPAL} = 10^{26}$) therefore succeed and set `epoch.junior_total = 1.2e26 > MAX_PRINCIPAL`. Once `epoch.junior_total > MAX_PRINCIPAL`:
    1. Every subsequent Senior deposit reverts with `Error::InvalidParameter` (because `check_senior_deposit` calls `validate_principal(epoch.junior_total)`).
    2. `senior_room()` silently returns `0` (`max_senior_total` returns `None`, turned into `0` by `unwrap_or(0)`).
    3. `position_of()` while `Open` silently returns `estimated_payout: 0` (`project_position` swallows `Err(_)` from `strata_waterfall::settle`).
    4. At maturity, **`settle()` permanently reverts with `Error::InvalidParameter`** (`strata_waterfall::settle` calls `validate_principal(epoch.junior_total)` at `contracts/waterfall/src/lib.rs:179`, returning `Err(WaterfallError::PrincipalTooLarge)`), locking all depositor funds forever.
- **Worked numbers:**
  - Deposit 1: `deposit(j1, Tranche::Junior, 60_000_000_000_000_000_000_000_000)` ($6 \times 10^{25} \le 10^{26}$) $\to$ `Ok`, `epoch.junior_total = 6e25`.
  - Deposit 2: `deposit(j2, Tranche::Junior, 60_000_000_000_000_000_000_000_000)` ($6 \times 10^{25} \le 10^{26}$) $\to$ `Ok`, `epoch.junior_total = 1.2e26 > MAX_PRINCIPAL`.
  - At maturity: `strata_waterfall::settle(0, 1.2e26, 500, 31_536_000, V)` returns `Err(WaterfallError::PrincipalTooLarge)`, which `EpochManager::settle` maps to `Err(Error::InvalidParameter)`.
- **Reproduction:**
  ```rust
  #[test]
  fn f2_cumulative_junior_deposits_exceeding_max_principal_brick_settle() {
      let d1: i128 = 60_000_000_000_000_000_000_000_000; // 6e25 <= MAX_PRINCIPAL (1e26)
      let d2: i128 = 60_000_000_000_000_000_000_000_000; // 6e25 <= MAX_PRINCIPAL (1e26)
      assert!(strata_waterfall::validate_principal(d1).is_ok());
      assert!(strata_waterfall::validate_principal(d2).is_ok());

      let junior_total = d1.checked_add(d2).unwrap(); // 1.2e26, accepted by EpochManager::deposit
      assert_eq!(
          strata_waterfall::settle(0, junior_total, 500, 31_536_000, junior_total),
          Err(strata_waterfall::WaterfallError::PrincipalTooLarge)
      );
  }
  ```
- **Remediation:** Call `validate_principal(new_junior).map_err(map_waterfall)?` on the post-deposit `epoch.junior_total` in `EpochManager::deposit` (`contracts/epoch-manager/src/lib.rs:346-351`).

---

### Finding F3 — **[Low, `bug`]** `pub fn senior_gate_holds` omits input validation and uses unchecked multiplication

- **Labels:** `bug`
- **Where:** `contracts/waterfall/src/lib.rs:256-264`.
- **Description:** Every other public function in `strata_waterfall` (`senior_due`, `settle`, `check_senior_deposit`, `max_senior_total`, `check_ratio`, `validate_principal`, `check_rate`, `check_term`) validates its inputs and uses `checked_mul` / `checked_add`. By contrast, `pub fn senior_gate_holds`:
  ```rust
  pub fn senior_gate_holds(
      senior_total: i128,
      junior_total: i128,
      max_senior_ratio_bps: u32,
  ) -> bool {
      let lhs = senior_total * (BPS_DENOMINATOR as i128);
      let rhs = junior_total * (max_senior_ratio_bps as i128);
      lhs <= rhs
  }
  ```
  1. **Unchecked overflow panic:** Calling `senior_gate_holds(i128::MAX, 1, 10_000)` panics (`attempt to multiply with overflow`) rather than returning `false` or `Result`, and in any downstream consumer compiled without `overflow-checks = true` it would silently wrap to a negative `i128` and return `true`.
  2. **Negative `senior_total` returns `true`:** Calling `senior_gate_holds(-1_000, 0, 10_000)` computes `lhs = -10_000_000 <= 0 = rhs` and returns `true`.
- **Reproduction:**
  ```rust
  #[test]
  fn f3_senior_gate_holds_accepts_negative_senior_and_panics_on_unvalidated_i128() {
      // Negative senior principal against zero junior capital returns `true`:
      assert!(strata_waterfall::senior_gate_holds(-1_000, 0, 10_000));

      // Unvalidated i128::MAX panics in `senior_total * 10_000`:
      let panicked = std::panic::catch_unwind(|| {
          strata_waterfall::senior_gate_holds(i128::MAX, 1, 10_000)
      });
      assert!(panicked.is_err());
  }
  ```

---

### Finding F4 — **[Low, `test`]** Property test coverage gaps in `tests/src/waterfall_props.rs`

- **Labels:** `test`
- **Where:** `tests/src/waterfall_props.rs:97-118, 154-169, 197-208, 236-252, 261-316`.
- **Description:**
  1. **`invariant_2_is_saturated_for_any_healthy_epoch` (`lines 197-208`) does not test saturation or healthy epochs:** It draws `v in small_value()` (`0..=1_000_000`) against `s in principal()` (up to `10^26`) and only asserts `g.senior_payout <= g.senior_due` and `g.senior_payout <= v`. It never asserts `if v >= g.senior_due { prop_assert_eq!(g.senior_payout, g.senior_due); }`.
  2. **`value()` (`lines 97-105`) caps `V` at `1000 * MAX_PRINCIPAL` (`10^29`):** Spec §2 defines the domain of `V` as `0 <= V` (all non-negative `i128` up to `i128::MAX = 1.7014e38`), but neither `waterfall_props.rs` nor `contracts/waterfall/src/tests.rs` tests `V = i128::MAX`.
  3. **`delta()` (`lines 116-118`) rarely tests `d == 0`:** `invariant_1_holds_near_the_cap_boundary` and `invariant_3_holds_at_the_boundary` add `d in -1_000_000..=1_000_000` to `due`. Because `delta()` does not include `Just(0)` in a `prop_oneof!`, the exact `V == senior_due` boundary (`d == 0`) is hit with probability `1 / 2,000,001` per random draw (`0.013%` across 256 cases).
  4. **Monotonicity across the kink (`lines 261-316`):** `invariant_4_both_payouts_are_monotone_in_v` tests `[v1, v1 + extra]` with `extra <= 10^9`, which almost never crosses `V = senior_due` when `S > 10^{12}`.

---

### Finding F5 — **[Low, `docs` / `bug`]** Inaccurate doc comments on `strata_waterfall::settle` and `epoch_manager::map_waterfall`, and `WaterfallError::Overflow` mapped to `Error::InvalidParameter`

- **Labels:** `bug`
- **Where:** `contracts/waterfall/src/lib.rs:165-166` and `contracts/epoch-manager/src/lib.rs:871-885`.
- **Description:**
  1. `contracts/waterfall/src/lib.rs:165-166` documents `junior_principal` in `settle` as:
     > *"Checked for validity and for the buffer invariant, but does not otherwise affect the split"*
     In reality, `settle` only calls `validate_principal(junior_principal)` (`0 <= J <= MAX_PRINCIPAL`) and **never** checks the buffer invariant `S <= J` (`settle(MAX_PRINCIPAL, 0, 500, 31_536_000, 0)` returns `Ok(...)`).
  2. `contracts/epoch-manager/src/lib.rs:871-885` documents `map_waterfall` as:
     > *"The variants line up one for one, so a rejection points at a specific clause of the spec rather than a generic failure."*
     In reality, 5 of the 7 `WaterfallError` variants (`PrincipalTooLarge`, `RateTooHigh`, `TermOutOfRange`, `RatioOutOfRange`, and `Overflow`) are mapped to the same `Error::InvalidParameter (9)` variant, and `WaterfallError::Overflow` is mapped to `Error::InvalidParameter (9)` rather than `Error::ArithmeticOverflow (12)`.

---

## 6. What this review did not cover

To state the boundaries of this review plainly:

1. **Wasm compilation and host VM execution:** This review inspected the Rust source of `contracts/waterfall`, `tests/src/waterfall_props.rs`, and the caller sites in `contracts/epoch-manager`, and verified the arithmetic in native Rust (`rustc 1.98.0`). It did not disassemble compiled `wasm32v1-none` binaries or verify the Soroban host VM's instruction semantics.
2. **Underlying vault implementations (`mock-vault`, `blend-adapter`, `blend-pool-mock`):** This review treated `V` (`value_redeemed`) as an exogenous `i128` input to the waterfall, as `docs/waterfall-spec.md` §2 defines it. It did not review share-price rounding or bToken conversion inside `mock-vault` or `blend-adapter`.
3. **Time-weighting and economic game theory around deposit/settlement timing:** `docs/waterfall-spec.md` defines `senior_due` on the epoch's fixed `term_seconds` regardless of when inside the deposit window a senior deposit arrived (tracked separately in issue #32 and R19 in `docs/risks.md`). This review checked conformance to `docs/waterfall-spec.md`, not whether time-unweighted interest is the right economic design.
4. **Formal verification (SMT / Kani / Coq):** The overflow and invariant proofs in Sections 2–4 were carried out by algebraic derivation and property-test inspection rather than a machine-checked SMT solver.
