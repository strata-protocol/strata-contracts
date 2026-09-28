//! Unit tests for the settlement math.
//!
//! The exhaustive property tests that guard the five spec invariants live in
//! `tests/waterfall_props.rs`. These are the readable, named cases: one per row
//! of the spec's worked-examples table, plus the boundary cases that a
//! randomly generated input will very rarely hit.

use std::panic;

use crate::{
    check_senior_deposit, max_senior_total, senior_due, senior_gate_holds, settle, Settlement,
    WaterfallError, BPS_DENOMINATOR, MAX_EPOCH_TERM_SECONDS, MAX_PRINCIPAL, MAX_SENIOR_RATE_BPS,
    MAX_SENIOR_RATIO_BPS, SECONDS_PER_YEAR, SECONDS_PER_YEAR_DENOMINATOR,
};

/// `S = J = 1_000_000`, `r = 500` bps/yr, `t = 1 year`, so `I = 50_000` and
/// `senior_due = 1_050_000`. Matches the spec's worked-examples table.
const S: i128 = 1_000_000;
const J: i128 = 1_000_000;
const R: u32 = 500;
const T: u64 = SECONDS_PER_YEAR;

fn expect(v: i128) -> Settlement {
    settle(S, J, R, T, v).expect("inputs are inside the documented limits")
}

#[test]
fn constants_match_the_spec() {
    assert_eq!(SECONDS_PER_YEAR, 31_536_000);
    assert_eq!(SECONDS_PER_YEAR_DENOMINATOR, 315_360_000_000);
    assert_eq!(BPS_DENOMINATOR, 10_000);
    assert_eq!(MAX_SENIOR_RATE_BPS, 10_000);
    assert_eq!(MAX_EPOCH_TERM_SECONDS, SECONDS_PER_YEAR);
    assert_eq!(MAX_SENIOR_RATIO_BPS, 10_000);
    assert_eq!(MAX_PRINCIPAL, 100_000_000_000_000_000_000_000_000);
}

#[test]
fn senior_due_matches_the_spec_worked_value() {
    // Spec section 4: S * r * t = 1.5768e16, / DENOM = 50_000.
    assert_eq!(senior_due(S, R, T).unwrap(), 1_050_000);
    assert_eq!(senior_due(S, R, T).unwrap(), S + 50_000);
}

#[test]
fn senior_interest_never_exceeds_principal_within_the_bounds() {
    // The buffer-coverage argument: I <= S holds because r <= 100%/yr and
    // t <= 1 year. Check the extreme corner, where the product is largest.
    assert_eq!(
        senior_due(S, MAX_SENIOR_RATE_BPS, MAX_EPOCH_TERM_SECONDS).unwrap(),
        2 * S
    );
    assert_eq!(senior_due(S, 0, T).unwrap(), S);
    assert_eq!(senior_due(0, MAX_SENIOR_RATE_BPS, T).unwrap(), 0);
}

/// One test per row of the spec's worked-examples table in section 9.
#[test]
fn worked_examples_from_the_spec() {
    let cases: &[(i128, i128, i128)] = &[
        // (V, senior_payout, junior_payout)
        (2_200_000, 1_050_000, 1_150_000), // healthy
        (1_050_000, 1_050_000, 0),         // exact hit, at the cushion limit
        (1_100_000, 1_050_000, 50_000),    // buffer absorbs the loss
        (1_049_999, 1_049_999, 0),         // just past the cushion, by 1 unit
        (900_000, 900_000, 0),             // cushion gone
        (0, 0, 0),                         // wipeout
    ];

    for &(v, want_senior, want_junior) in cases {
        let got = expect(v);
        assert_eq!(
            (got.senior_payout, got.junior_payout),
            (want_senior, want_junior),
            "V = {v}"
        );
        // Invariant 1, on the spec's own numbers.
        assert_eq!(got.senior_payout + got.junior_payout, v, "V = {v}");
    }
}

/// The senior is made whole against losses of at most `J - I`, and by exactly
/// one base unit less than that. The single sharpest behavioural boundary in
/// the whole spec, and the one most likely to regress.
#[test]
fn the_cushion_boundary_is_sharp() {
    let cushion = J - (senior_due(S, R, T).unwrap() - S); // J - I = 950_000
    let total = S + J;
    let value_at_cushion = total - cushion; // 1_050_000

    assert_eq!(cushion, 950_000);
    assert_eq!(expect(value_at_cushion).senior_payout, 1_050_000);
    assert_eq!(expect(value_at_cushion).junior_payout, 0);

    let just_past = expect(value_at_cushion - 1);
    assert_eq!(just_past.senior_payout, 1_049_999);
    assert_eq!(just_past.junior_payout, 0);
}

/// Invariant 2 at the boundary: the senior is capped even when the underlying
/// does far better than the target.
#[test]
fn senior_is_capped_in_a_very_good_epoch() {
    let got = expect(1_000_000_000);
    assert_eq!(got.senior_payout, got.senior_due);
    assert_eq!(got.senior_payout, 1_050_000);
    assert_eq!(got.junior_payout, 1_000_000_000 - 1_050_000);
}

#[test]
fn senior_payout_never_exceeds_senior_due() {
    for v in [0, 1, 1_049_999, 1_050_000, 1_050_001, i64::MAX as i128] {
        let got = expect(v);
        assert!(got.senior_payout <= got.senior_due, "V = {v}");
    }
}

#[test]
fn payouts_are_non_negative() {
    for v in [0, 1, 1_050_000, 12_345] {
        let got = expect(v);
        assert!(got.senior_payout >= 0, "V = {v}");
        assert!(got.junior_payout >= 0, "V = {v}");
    }
}

#[test]
fn a_huge_junior_tranche_does_not_disturb_the_split() {
    // J never enters the arithmetic. A 1e26 junior buffer must produce the
    // same senior figure as an empty one, otherwise S and J have been
    // conflated somewhere.
    let small = settle(S, 1, R, T, 1_050_000).unwrap();
    let huge = settle(S, MAX_PRINCIPAL, R, T, 1_050_000).unwrap();
    assert_eq!(small, huge);
}

#[test]
fn rejects_inputs_outside_the_documented_limits() {
    assert_eq!(
        settle(MAX_PRINCIPAL + 1, J, R, T, 0),
        Err(WaterfallError::PrincipalTooLarge)
    );
    assert_eq!(
        settle(S, MAX_PRINCIPAL + 1, R, T, 0),
        Err(WaterfallError::PrincipalTooLarge)
    );
    assert_eq!(
        settle(S, J, MAX_SENIOR_RATE_BPS + 1, T, 0),
        Err(WaterfallError::RateTooHigh)
    );
    assert_eq!(
        settle(S, J, R, MAX_EPOCH_TERM_SECONDS + 1, 0),
        Err(WaterfallError::TermOutOfRange)
    );
    assert_eq!(settle(S, J, R, 0, 0), Err(WaterfallError::TermOutOfRange));
    assert_eq!(settle(-1, J, R, T, 0), Err(WaterfallError::NegativeAmount));
    assert_eq!(settle(S, J, R, T, -1), Err(WaterfallError::NegativeAmount));
}

#[test]
fn accepts_every_documented_limit() {
    // The boundary case that invariant 5 leans on: MAX_PRINCIPAL at the
    // maximum rate and maximum term must settle, not overflow.
    let got = settle(
        MAX_PRINCIPAL,
        MAX_PRINCIPAL,
        MAX_SENIOR_RATE_BPS,
        MAX_EPOCH_TERM_SECONDS,
        0,
    );
    assert_eq!(got.unwrap().senior_payout, 0);

    // And the largest documentable senior_due at all.
    let due = senior_due(MAX_PRINCIPAL, MAX_SENIOR_RATE_BPS, MAX_EPOCH_TERM_SECONDS).unwrap();
    assert_eq!(due, 2 * MAX_PRINCIPAL);
}

// --- deposit gate, spec section 7 ---

#[test]
fn senior_deposits_are_blocked_while_junior_is_empty() {
    // A first senior deposit of d into an empty epoch leaves S = d, J = 0.
    // The ratio half requires d * 10_000 <= 0, which is false for d > 0, so
    // the ratio rule alone already blocks an unbacked senior deposit. Only a
    // zero-amount no-op slips past the ratio half; the redundant `J > 0`
    // condition in `check_senior_deposit` rejects that too.
    for d in [1i128, 2, 500_000, MAX_PRINCIPAL] {
        assert!(
            !senior_gate_holds(d, 0, MAX_SENIOR_RATIO_BPS),
            "an unbacked senior deposit of {d} must not pass the ratio half"
        );
    }

    // The only pair the ratio half admits with J = 0 is the vacuous one.
    assert!(senior_gate_holds(0, 0, MAX_SENIOR_RATIO_BPS));

    assert_eq!(
        check_senior_deposit(0, 0, MAX_SENIOR_RATIO_BPS),
        Err(WaterfallError::SeniorGateViolated)
    );
    assert_eq!(
        check_senior_deposit(500_000, 0, MAX_SENIOR_RATIO_BPS),
        Err(WaterfallError::SeniorGateViolated)
    );
}

#[test]
fn a_junior_deposit_unblocks_exactly_the_configured_room() {
    // ratio 1.0x and J = 1_000_000 admits S_total up to 1_000_000, inclusive.
    assert!(senior_gate_holds(
        1_000_000,
        1_000_000,
        MAX_SENIOR_RATIO_BPS
    ));
    assert!(!senior_gate_holds(
        1_000_001,
        1_000_000,
        MAX_SENIOR_RATIO_BPS
    ));

    assert_eq!(check_senior_deposit(1_000_000, 1_000_000, 10_000), Ok(()));
    assert_eq!(
        check_senior_deposit(1_000_001, 1_000_000, 10_000),
        Err(WaterfallError::SeniorGateViolated)
    );
}

#[test]
fn a_smaller_ratio_admits_proportionally_less_senior() {
    // 0.5x: S <= J / 2.
    assert!(senior_gate_holds(500_000, 1_000_000, 5_000));
    assert!(!senior_gate_holds(500_001, 1_000_000, 5_000));
}

#[test]
fn the_gate_agrees_with_the_reported_room() {
    // `max_senior_total` floors; the gate is cross-multiplied. These must
    // agree exactly, or the dashboard would advertise room that the contract
    // then rejects. The two forms are equivalent only because `S` is an
    // integer, which is worth pinning.
    for ratio in [1u32, 3, 2_000, 5_000, 9_999, MAX_SENIOR_RATIO_BPS] {
        for j in [0i128, 1, 2, 3, 7, 1_000, 1_000_000] {
            let room = max_senior_total(j, ratio).expect("ratio and j are in range");
            assert!(
                senior_gate_holds(room, j, ratio),
                "ratio={ratio} j={j}: the reported room {room} must be admissible"
            );
            if room < MAX_PRINCIPAL {
                assert!(
                    !senior_gate_holds(room + 1, j, ratio),
                    "ratio={ratio} j={j}: {room} + 1 must exceed the room"
                );
            }
        }
    }
}

#[test]
fn max_senior_total_reports_the_remaining_room() {
    assert_eq!(max_senior_total(1_000_000, 10_000), Some(1_000_000));
    assert_eq!(max_senior_total(1_000_000, 5_000), Some(500_000));
    assert_eq!(max_senior_total(0, 10_000), Some(0));

    // Reports the largest *depositable* integer, so it floors.
    assert_eq!(max_senior_total(3, 5_000), Some(1)); // 1.5 -> 1
    assert_eq!(max_senior_total(3, 10_000), Some(3));

    // Out of contract: a zero or oversized ratio reports no room rather than
    // a misleading number.
    assert_eq!(max_senior_total(1_000_000, 0), None);
    assert_eq!(max_senior_total(1_000_000, MAX_SENIOR_RATIO_BPS + 1), None);
    assert_eq!(max_senior_total(-1, 10_000), None);
}

#[test]
fn the_gate_is_monotone() {
    // The gate must be antitone in senior capital, monotone in junior
    // capital, and antitone in the ratio: adding senior or tightening the
    // ratio can only ever reject, adding junior or loosening the ratio can
    // only ever accept.
    for ratio in [1u32, 2_000, 5_000, MAX_SENIOR_RATIO_BPS] {
        for j in [1i128, 3, 1_000, 1_000_000] {
            for s in [0i128, 1, 13, 999_999] {
                if senior_gate_holds(s + 1, j, ratio) {
                    assert!(senior_gate_holds(s, j, ratio), "s={s} j={j} r={ratio}");
                }
                if senior_gate_holds(s, j, ratio) {
                    assert!(senior_gate_holds(s, j + 1, ratio), "s={s} j={j} r={ratio}");
                    assert!(senior_gate_holds(s, j, ratio + 1), "s={s} j={j} r={ratio}");
                }
            }
        }
    }
}

#[test]
fn at_a_1x_ratio_senior_principal_may_equal_junior_principal() {
    // At `max_senior_ratio_bps == 10_000` the gate is exactly `S <= J`.
    for (s, j) in [(0, 1), (1, 1), (2, 2), (1_000_000, 1_000_000)] {
        assert!(senior_gate_holds(s, j, MAX_SENIOR_RATIO_BPS), "s={s} j={j}");
    }
    for (s, j) in [(2, 1), (1_000_001, 1_000_000)] {
        assert!(
            !senior_gate_holds(s, j, MAX_SENIOR_RATIO_BPS),
            "s={s} j={j}"
        );
    }
}

#[test]
fn overflow_checks_are_enabled_in_this_profile() {
    // Guards the profile config in the workspace Cargo.toml. If this fails,
    // `overflow-checks` has been switched off and the CVE-2026-24889 class of
    // silent-wrap bugs is back.
    let result = panic::catch_unwind(|| {
        MAX_PRINCIPAL
            .checked_mul(MAX_PRINCIPAL)
            .expect("MAX_PRINCIPAL squared must not fit in i128")
    });
    assert!(result.is_err(), "MAX_PRINCIPAL^2 unexpectedly fit in i128");
}
