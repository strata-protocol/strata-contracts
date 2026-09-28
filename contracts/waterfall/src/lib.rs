//! Senior/junior settlement math for Strata.
//!
//! This is a translation of [`docs/waterfall-spec.md`](../../docs/waterfall-spec.md)
//! and nothing else. It has no `Env`, no storage, and no dependency on
//! `soroban-sdk`: the whole of Strata's money-handling logic is integer
//! arithmetic on `i128`, and keeping it in a plain Rust crate means the
//! property tests need no ledger and run in milliseconds.
//!
//! # What it does
//!
//! Given the senior principal `S`, the junior principal `J`, a target rate
//! `r` in basis points per year, an epoch length `t` in seconds, and the
//! value `V` actually redeemed from the underlying vault, it splits `V`
//! between the two tranches.
//!
//! # What it does not do
//!
//! It does not enforce the deposit gate, track shares, move tokens, or know
//! about time. See [`epoch-manager`](https://github.com/strata-finance/strata-contracts)
//! for all of that. This crate answers exactly one question: given these five
//! numbers, who gets paid what?

#![no_std]

/// Seconds in a 365-day year. Fixed by the spec so results never depend on
/// leap years, daylight saving, or the host clock.
pub const SECONDS_PER_YEAR: u64 = 31_536_000;

/// Basis-point denominator: 10_000 == 100% == 1.0x.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Denominator of the senior interest term, precomputed as a `u64` so the
/// settlement path never has to widen a product before dividing.
pub const SECONDS_PER_YEAR_DENOMINATOR: u64 = BPS_DENOMINATOR as u64 * SECONDS_PER_YEAR;

/// Maximum senior target rate, in basis points per year. 10_000 == 100%/yr.
///
/// Bounding the rate at 100%/yr is what makes the junior buffer provably cover
/// the senior's target interest. See "Why these bounds" in the spec.
pub const MAX_SENIOR_RATE_BPS: u32 = 10_000;

/// Maximum epoch length in seconds. One year.
///
/// Bounded so that `r * t / SECONDS_PER_YEAR` can never exceed 1.0, which is
/// the other half of the buffer-coverage argument.
pub const MAX_EPOCH_TERM_SECONDS: u64 = SECONDS_PER_YEAR;

/// Maximum `max_senior_ratio_bps` at epoch creation. 10_000 == 1.0x, i.e.
/// senior principal may at most equal junior principal.
pub const MAX_SENIOR_RATIO_BPS: u32 = 10_000;

/// Maximum principal in a single tranche: 1e26 base units.
///
/// Derived from the overflow headroom of `S * r * t`, not chosen for looks.
/// With `r <= 10_000` and `t <= 31_536_000` the largest intermediate is
/// `1e26 * 3.1536e11 == 3.1536e37`, against `i128::MAX == 1.7014e38`, so
/// roughly 5.4x headroom remains.
pub const MAX_PRINCIPAL: i128 = 100_000_000_000_000_000_000_000_000;

/// The result of settling one epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// `S + (S * r * t) / (10_000 * SECONDS_PER_YEAR)`, the senior's target.
    pub senior_due: i128,
    /// What the senior actually receives. Always `<= senior_due`.
    pub senior_payout: i128,
    /// What the junior actually receives. `V - senior_payout`.
    pub junior_payout: i128,
}

/// Everything that can go wrong in the settlement path.
///
/// Each variant corresponds to a documented limit in the spec, so a
/// `WaterfallError` in a log always points at the clause that was violated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterfallError {
    /// A principal, or a redeemed value, was negative. The value domain is
    /// `V >= 0`; a negative redemption means the underlying vault is broken.
    NegativeAmount,
    /// A principal exceeded [`MAX_PRINCIPAL`].
    PrincipalTooLarge,
    /// The senior target rate exceeded [`MAX_SENIOR_RATE_BPS`].
    RateTooHigh,
    /// The epoch term was zero or exceeded [`MAX_EPOCH_TERM_SECONDS`].
    TermOutOfRange,
    /// `max_senior_ratio_bps` was zero or exceeded [`MAX_SENIOR_RATIO_BPS`].
    RatioOutOfRange,
    /// `S * r * t` did not fit in an `i128`, or the final addition
    /// `S + senior_interest` did. Unreachable while every input is inside its
    /// documented limit; present so that a caller which skipped validation
    /// gets a trap rather than a wrapped number.
    Overflow,
    /// A senior deposit was rejected by the gate: `J_total > 0` was not
    /// satisfied, or `S_total * 10_000 > J_total * max_senior_ratio_bps`.
    /// See spec section 7.
    SeniorGateViolated,
}

impl core::fmt::Display for WaterfallError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let msg = match self {
            Self::NegativeAmount => "amount is negative",
            Self::PrincipalTooLarge => "principal exceeds MAX_PRINCIPAL",
            Self::RateTooHigh => "senior rate exceeds MAX_SENIOR_RATE_BPS",
            Self::TermOutOfRange => "epoch term is zero or exceeds MAX_EPOCH_TERM_SECONDS",
            Self::RatioOutOfRange => "max_senior_ratio_bps is zero or exceeds MAX_SENIOR_RATIO_BPS",
            Self::Overflow => "settlement arithmetic overflowed",
            Self::SeniorGateViolated => "senior deposit rejected by the junior buffer gate",
        };
        f.write_str(msg)
    }
}

/// The senior's target: principal plus one term of simple interest at `r`.
///
/// ```text
/// senior_due = S + (S * r * t) / (10_000 * SECONDS_PER_YEAR)
/// ```
///
/// The division truncates toward zero, so any sub-base-unit fraction is
/// retained by the protocol and flows to the junior at settlement. This is the
/// only rounding in the settlement path.
pub fn senior_due(
    senior_principal: i128,
    rate_bps: u32,
    term_seconds: u64,
) -> Result<i128, WaterfallError> {
    validate_common(senior_principal, rate_bps, term_seconds)?;

    // Widen the rate and term to i128 before multiplying so the product cannot
    // wrap. `i128 * u32` and `i128 * u64` are already checked, but widening
    // both factors explicitly makes the intent obvious to the next reader.
    let r = rate_bps as i128;
    let t = term_seconds as i128;

    let numerator = senior_principal
        .checked_mul(r)
        .and_then(|p| p.checked_mul(t))
        .ok_or(WaterfallError::Overflow)?;

    let interest = numerator
        .checked_div(SECONDS_PER_YEAR_DENOMINATOR as i128)
        .ok_or(WaterfallError::Overflow)?;

    senior_principal
        .checked_add(interest)
        .ok_or(WaterfallError::Overflow)
}

/// Splits `value_redeemed` between the two tranches. Spec section 5.
///
/// ```text
/// senior_payout = min(V, senior_due)
/// junior_payout = V - senior_payout
/// ```
///
/// The senior is capped at its target and paid first; the junior takes the
/// remainder, which in a bad epoch is nothing. The split introduces no
/// rounding of its own, so `senior_payout + junior_payout == V` always holds
/// exactly.
///
/// # Arguments
///
/// - `senior_principal` — `S`, total senior principal in the epoch.
/// - `junior_principal` — `J`, total junior principal. Checked for validity
///   and for the buffer invariant, but does not otherwise affect the split:
///   by the time this runs, `V` already contains everything.
/// - `rate_bps` — `r`, the senior target rate.
/// - `term_seconds` — `t`, the epoch length.
/// - `value_redeemed` — `V`, the assets actually redeemed at maturity.
pub fn settle(
    senior_principal: i128,
    junior_principal: i128,
    rate_bps: u32,
    term_seconds: u64,
    value_redeemed: i128,
) -> Result<Settlement, WaterfallError> {
    validate_common(senior_principal, rate_bps, term_seconds)?;
    validate_principal(junior_principal)?;

    if value_redeemed < 0 {
        return Err(WaterfallError::NegativeAmount);
    }

    let due = senior_due(senior_principal, rate_bps, term_seconds)?;

    // min(V, due) via a branch rather than i128::min, to keep this readable
    // as the two-branch form the spec is written in.
    let senior_payout = if value_redeemed >= due {
        due
    } else {
        value_redeemed
    };

    let junior_payout = value_redeemed - senior_payout;

    Ok(Settlement {
        senior_due: due,
        senior_payout,
        junior_payout,
    })
}

/// Decides whether a senior deposit may be accepted. Spec section 7.
///
/// `senior_total` and `junior_total` are the tranche totals *after* the
/// pending deposit has been applied.
///
/// # The `junior_total > 0` condition is redundant
///
/// It was added on the belief that `S <= J * ratio` is vacuously true on an
/// empty epoch and would admit an unbacked first senior deposit. That belief
/// was wrong: because the totals are post-deposit, a first senior deposit of
/// `d` leaves `S = d, J = 0`, and the ratio half then requires
/// `d * 10_000 <= 0`, which is false for every `d > 0`.
///
/// It is kept as defense-in-depth. It states the "junior buffer is always
/// real" invariant directly rather than leaving it to be derived from the
/// algebra, and it remains load-bearing if the ratio rule is ever relaxed past
/// [`MAX_SENIOR_RATIO_BPS`], at which point the ratio half no longer implies
/// `S <= J`. Its only observable effect is to reject a zero-amount deposit.
pub fn check_senior_deposit(
    senior_total: i128,
    junior_total: i128,
    max_senior_ratio_bps: u32,
) -> Result<(), WaterfallError> {
    validate_principal(senior_total)?;
    validate_principal(junior_total)?;
    check_ratio(max_senior_ratio_bps)?;

    if junior_total <= 0 {
        return Err(WaterfallError::SeniorGateViolated);
    }

    senior_gate_holds(senior_total, junior_total, max_senior_ratio_bps)
        .then_some(())
        .ok_or(WaterfallError::SeniorGateViolated)
}

/// The ratio half of the deposit gate, without the `junior_total > 0` check.
///
/// Returns `true` when `S <= J * ratio` holds. Both cross terms are computed
/// in `i128` and are bounded by `1e30`, so neither can overflow.
///
/// The comparison is written cross-multiplied as
/// `S * 10_000 <= J * ratio_bps` rather than `S <= J * ratio_bps / 10_000` so
/// that it mirrors the spec inequality literally and needs no division. The
/// two forms are exactly equivalent: `S` is an integer, and `S <= x` is
/// equivalent to `S <= floor(x)` for integer `S`. A property test pins that
/// equivalence so a future edit cannot quietly change it.
///
/// Note this function does **not** apply the `junior_total > 0` condition, so
/// it returns `true` for `(0, 0, _)`. That vacuous case is precisely why
/// [`check_senior_deposit`] exists; call this one only when you already know
/// the junior tranche is non-empty.
pub fn senior_gate_holds(
    senior_total: i128,
    junior_total: i128,
    max_senior_ratio_bps: u32,
) -> bool {
    let lhs = senior_total * (BPS_DENOMINATOR as i128);
    let rhs = junior_total * (max_senior_ratio_bps as i128);
    lhs <= rhs
}

/// The largest senior total the gate would admit for a given junior total.
///
/// Returns `None` when the ratio or the junior principal is outside the
/// documented limits, or when the answer would exceed [`MAX_PRINCIPAL`] and
/// therefore could not be deposited anyway. This is what the dashboard uses
/// to show a depositor how much senior room is left.
pub fn max_senior_total(junior_total: i128, max_senior_ratio_bps: u32) -> Option<i128> {
    check_ratio(max_senior_ratio_bps).ok()?;
    validate_principal(junior_total).ok()?;

    let room = junior_total.checked_mul(max_senior_ratio_bps as i128)? / (BPS_DENOMINATOR as i128);

    (room <= MAX_PRINCIPAL).then_some(room)
}

/// Validates a `max_senior_ratio_bps` supplied at epoch creation.
pub fn check_ratio(max_senior_ratio_bps: u32) -> Result<(), WaterfallError> {
    if max_senior_ratio_bps == 0 || max_senior_ratio_bps > MAX_SENIOR_RATIO_BPS {
        return Err(WaterfallError::RatioOutOfRange);
    }
    Ok(())
}

/// Validates a principal against the documented ceiling.
pub fn validate_principal(principal: i128) -> Result<(), WaterfallError> {
    if principal < 0 {
        return Err(WaterfallError::NegativeAmount);
    }
    if principal > MAX_PRINCIPAL {
        return Err(WaterfallError::PrincipalTooLarge);
    }
    Ok(())
}

/// Shared validation for the three rate/term/principal parameters.
fn validate_common(
    principal: i128,
    rate_bps: u32,
    term_seconds: u64,
) -> Result<(), WaterfallError> {
    validate_principal(principal)?;
    check_rate(rate_bps)?;
    check_term(term_seconds)?;
    Ok(())
}

/// Validates a senior target rate against the documented ceiling.
pub fn check_rate(rate_bps: u32) -> Result<(), WaterfallError> {
    if rate_bps > MAX_SENIOR_RATE_BPS {
        return Err(WaterfallError::RateTooHigh);
    }
    Ok(())
}

/// Validates an epoch term against the documented bounds.
pub fn check_term(term_seconds: u64) -> Result<(), WaterfallError> {
    if term_seconds == 0 || term_seconds > MAX_EPOCH_TERM_SECONDS {
        return Err(WaterfallError::TermOutOfRange);
    }
    Ok(())
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
