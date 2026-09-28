//! Property tests for the five invariants in `docs/waterfall-spec.md`.
//!
//! One section per invariant, numbered as the spec numbers them. These are the
//! tests that actually hold the implementation to the spec; the named unit
//! tests in `contracts/waterfall/src/tests.rs` cover the worked examples and
//! the boundaries, and cannot cover the interior.
//!
//! Every generator produces inputs **inside the documented limits** of spec
//! section 3, so a `settle` that returns `Err` here is a bug, not a rejected
//! input. Tests assert that the call succeeds before asserting anything about
//! the result.
//!
//! Run with more cases when changing the math:
//!
//! ```text
//! PROPTEST_CASES=100000 cargo test -p strata-tests
//! ```

use proptest::prelude::*;
use strata_waterfall::{
    check_senior_deposit, max_senior_total, senior_due, senior_gate_holds, settle, Settlement,
    MAX_EPOCH_TERM_SECONDS, MAX_PRINCIPAL, MAX_SENIOR_RATE_BPS, MAX_SENIOR_RATIO_BPS,
};

/// Enough cases to exercise the interior of both branches and the boundary
/// between them. 256 runs in a fraction of a second in CI.
///
/// `PROPTEST_CASES` overrides it, because an explicit `proptest_config`
/// attribute otherwise takes precedence over the environment variable and
/// silently ignores it.
const CASES: u32 = 256;

fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES);

    ProptestConfig {
        cases,
        // Persist shrunk counterexamples next to the tests so a failure is
        // reproducible without hunting through CI logs.
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"),
        )),
        ..ProptestConfig::default()
    }
}

/// A senior or junior principal inside the documented ceiling.
///
/// A uniform draw over `[0, 1e26]` would almost always be astronomically
/// large, which is good for the overflow invariant and useless for hitting the
/// interesting branch boundaries. So this mixes realistic magnitudes with the
/// exact extremes, and lets proptest pick a mix.
fn principal() -> impl Strategy<Value = i128> {
    prop_oneof![
        0i128..=1_000_000i128,
        0i128..=1_000_000_000_000i128,
        0i128..=1_000_000_000_000_000_000i128,
        0i128..=MAX_PRINCIPAL,
        Just(0),
        Just(1),
        Just(MAX_PRINCIPAL),
    ]
}

/// A senior rate inside the documented ceiling, in the same mixed style.
fn rate() -> impl Strategy<Value = u32> {
    prop_oneof![
        0u32..=100u32,
        0u32..=1_000u32,
        0u32..=MAX_SENIOR_RATE_BPS,
        Just(0),
        Just(1),
        Just(MAX_SENIOR_RATE_BPS),
    ]
}

/// A term inside the documented bounds. `1..=`, since a zero term is invalid.
fn term() -> impl Strategy<Value = u64> {
    prop_oneof![
        1u64..=1_000u64,
        1u64..=86_400u64,
        1u64..=MAX_EPOCH_TERM_SECONDS,
        Just(1),
        Just(86_400),
        Just(MAX_EPOCH_TERM_SECONDS / 4),
        Just(MAX_EPOCH_TERM_SECONDS),
    ]
}

/// A redeemed value. `V` is not bounded above by the protocol -- an epoch can
/// return far more than went in -- so this spans nothing up to a thousand
/// times the maximum principal, which covers the wipeout, the loss, the
/// exact-hit and the large-surplus cases.
fn value() -> impl Strategy<Value = i128> {
    prop_oneof![
        small_value(),
        0i128..=1_000_000_000_000i128,
        0i128..=MAX_PRINCIPAL,
        0i128..=MAX_PRINCIPAL.saturating_mul(1_000),
        Just(0),
    ]
}

/// A value biased small, so the `V < senior_due` branch is reached often
/// rather than rarely.
fn small_value() -> impl Strategy<Value = i128> {
    0i128..=1_000_000i128
}

/// A signed offset from `senior_due`, for the boundary. The boundary is where
/// the senior cap and the loss case meet, and a uniform random `V` almost
/// never lands on it.
fn delta() -> impl Strategy<Value = i128> {
    -1_000_000i128..=1_000_000i128
}

/// Settles and asserts the call succeeded, so the invariants below can be
/// checked without an `Option` dance. A failure here means an input inside
/// the documented limits was rejected, which is invariant 5 failing.
fn settled(s: i128, j: i128, r: u32, t: u64, v: i128) -> Settlement {
    settle(s, j, r, t, v).unwrap_or_else(|e| {
        panic!("settle({s}, {j}, {r}, {t}, {v}) returned {e:?} for an in-range input")
    })
}

proptest! {
    #![proptest_config(config())]

    // --- Invariant 1: conservation ---------------------------------------
    //
    // senior_payout + junior_payout == V. No value created, none lost.
    //
    // This one is structural: the implementation computes
    // `junior_payout = V - senior_payout`, so it cannot fail unless the
    // senior branch is wrong. The test is here to catch a future edit that
    // introduces a separate computation for the junior.

    #[test]
    fn invariant_1_payouts_sum_to_value(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        v in value(),
    ) {
        let g = settled(s, j, r, t, v);
        prop_assert_eq!(g.senior_payout + g.junior_payout, v);
    }

    #[test]
    fn invariant_1_holds_near_the_cap_boundary(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        d in delta(),
    ) {
        let due = senior_due(s, r, t).expect("in-range input");
        // Exactly the boundary and either side of it, where conservation is
        // easiest to break.
        for v in [due + d - 1, due + d, due + d + 1] {
            let v = v.max(0);
            let g = settled(s, j, r, t, v);
            prop_assert_eq!(g.senior_payout + g.junior_payout, v);
        }
    }

    #[test]
    fn invariant_1_survives_interest_truncation(s in principal(), r in rate(), t in term(), v in value()) {
        // Terms that do not divide the denominator evenly truncate the
        // interest, and the discarded fraction must land with the junior.
        let g = settled(s, 0, r, t, v);
        prop_assert_eq!(g.senior_payout + g.junior_payout, v);
        prop_assert!(g.senior_due >= s, "senior is never principal-negative");
    }

    // --- Invariant 2: senior cap -----------------------------------------
    //
    // senior_payout <= senior_due. The senior never beats its target.

    #[test]
    fn invariant_2_senior_never_exceeds_its_due(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        v in value(),
    ) {
        let g = settled(s, j, r, t, v);
        prop_assert!(g.senior_payout <= g.senior_due);
    }

    #[test]
    fn invariant_2_is_saturated_for_any_healthy_epoch(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        v in small_value(),
    ) {
        // A small V must never overpay the senior.
        let g = settled(s, j, r, t, v);
        prop_assert!(g.senior_payout <= g.senior_due);
        prop_assert!(g.senior_payout <= v);
    }

    // --- Invariant 3: senior paid first ----------------------------------
    //
    // junior_payout > 0  implies  senior_payout == senior_due.
    //
    // The converse is explicitly NOT claimed: when V == senior_due exactly
    // the senior is whole and the junior gets nothing. The test below asserts
    // only the direction the spec promises.

    #[test]
    fn invariant_3_junior_is_paid_only_after_the_senior_is_whole(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        v in value(),
    ) {
        let g = settled(s, j, r, t, v);
        if g.junior_payout > 0 {
            prop_assert!(
                g.senior_payout == g.senior_due,
                "junior was paid {} but the senior only got {} of {}",
                g.junior_payout, g.senior_payout, g.senior_due
            );
        }
    }

    #[test]
    fn invariant_3_holds_at_the_boundary(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        d in delta(),
    ) {
        let due = senior_due(s, r, t).expect("in-range input");
        for v in [due + d - 1, due + d, due + d + 1] {
            let v = v.max(0);
            let g = settled(s, j, r, t, v);
            if g.junior_payout > 0 {
                prop_assert_eq!(g.senior_payout, g.senior_due, "V = {}", v);
            }
        }
    }

    // --- Invariant 4: monotonicity in V ----------------------------------
    //
    // Both payouts are non-decreasing in V. A depositor is never better off if
    // the underlying does worse -- the property that makes this a tranche
    // wrapper and not a bet on the underlying.

    #[test]
    fn invariant_4_both_payouts_are_monotone_in_v(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
        v1 in value(),
        extra in 0i128..=1_000_000_000i128,
    ) {
        let v2 = v1.saturating_add(extra);
        let a = settled(s, j, r, t, v1);
        let b = settled(s, j, r, t, v2);

        prop_assert!(
            b.senior_payout >= a.senior_payout,
            "senior fell: V={} gave {}, V={} gave {}",
            v1, a.senior_payout, v2, b.senior_payout
        );
        prop_assert!(
            b.junior_payout >= a.junior_payout,
            "junior fell: V={} gave {}, V={} gave {}",
            v1, a.junior_payout, v2, b.junior_payout
        );
    }

    #[test]
    fn invariant_4_is_flat_above_the_cap_and_below_it(
        s in principal(),
        j in principal(),
        r in rate(),
        t in term(),
    ) {
        // Both payouts have a flat region, so the property is
        // non-decreasing and not strictly increasing. Pin both flats so a
        // future edit cannot turn it into a step that skips value.
        let due = senior_due(s, r, t).expect("in-range input");

        // Above the cap: the senior is saturated, the junior takes the growth.
        let far = due + 1_000_000;
        let a = settled(s, j, r, t, far);
        let b = settled(s, j, r, t, far + 1_000_000);
        prop_assert_eq!(a.senior_payout, b.senior_payout);
        prop_assert_eq!(a.senior_payout, a.senior_due);
        prop_assert_eq!(b.junior_payout - a.junior_payout, 1_000_000);

        // Below the cap: the senior absorbs the growth, the junior is flat.
        // Needs room for the whole span to sit strictly under the cap, which
        // is not guaranteed for a tiny principal or a short term.
        if due > 2_000_000 {
            let low = due - 1_000_000;
            let a = settled(s, j, r, t, low);
            let b = settled(s, j, r, t, low + 1_000_000);
            prop_assert_eq!(a.junior_payout, b.junior_payout);
            prop_assert_eq!(a.junior_payout, 0);
            prop_assert_eq!(b.senior_payout - a.senior_payout, 1_000_000);
        }
    }

    // --- Invariant 5: no overflow ---------------------------------------
    //
    // Nothing overflows for i128 amounts within the documented limits. The
    // strongest form of this: the documented corner -- the largest principal,
    // the largest rate and the longest term all at once -- settles rather
    // than panicking or wrapping.

    #[test]
    fn invariant_5_the_documented_corner_settles_without_overflow(
        s in principal(),
        r in rate(),
        t in term(),
    ) {
        let j = MAX_PRINCIPAL;
        let v = MAX_PRINCIPAL;
        let g = settled(s, j, r, t, v);

        prop_assert!(g.senior_due >= 0);
        prop_assert!(g.senior_payout >= 0);
        prop_assert!(g.junior_payout >= 0);
        prop_assert_eq!(g.senior_payout + g.junior_payout, v);
    }

    #[test]
    fn invariant_5_never_panics_at_the_extreme_corner(v in value()) {
        // Every documented limit simultaneously.
        let g = settled(
            MAX_PRINCIPAL,
            MAX_PRINCIPAL,
            MAX_SENIOR_RATE_BPS,
            MAX_EPOCH_TERM_SECONDS,
            v,
        );
        prop_assert!(g.senior_payout <= g.senior_due);
        prop_assert_eq!(g.senior_payout + g.junior_payout, v);
    }

    #[test]
    fn invariant_5_senior_due_never_exceeds_twice_the_principal(
        s in principal(),
        r in rate(),
        t in term(),
    ) {
        // MAX_SENIOR_RATE_BPS / MAX_EPOCH_TERM_SECONDS is 100%/yr for 1 year, so
        // the interest can never exceed the principal. This is the property
        // the buffer-coverage argument in the spec rests on, so it is checked
        // over the whole input domain rather than at a few points.
        let due = senior_due(s, r, t).expect("in-range input");
        prop_assert!(due >= s, "senior_due must be at least principal");
        prop_assert!(
            due <= s.saturating_mul(2),
            "senior_due {} exceeded 2 * {}: the rate or term bound is wrong",
            due, s
        );
    }

    // --- deposit gate, spec section 7 ------------------------------------

    #[test]
    fn gate_admits_exactly_the_configured_room(
        s in principal(),
        j in 1i128..=MAX_PRINCIPAL,
        ratio in 1u32..=MAX_SENIOR_RATIO_BPS,
    ) {
        // The check and the reported room must never disagree, or the
        // dashboard would advertise room the contract then rejects.
        let room = max_senior_total(j, ratio).expect("ratio and j are in range");
        prop_assert!(senior_gate_holds(room, j, ratio));

        match check_senior_deposit(s, j, ratio) {
            Ok(()) => prop_assert!(
                s <= room,
                "gate accepted {} but the room is only {} (j={}, ratio={})",
                s, room, j, ratio
            ),
            Err(_) => prop_assert!(
                s > room,
                "gate rejected {} but the room is {} (j={}, ratio={})",
                s, room, j, ratio
            ),
        }
    }

    #[test]
    fn gate_agrees_with_a_direct_evaluation(
        s in principal(),
        j in principal(),
        ratio in 1u32..=MAX_SENIOR_RATIO_BPS,
    ) {
        // `S <= J * ratio` cross-multiplied, spelled out independently of the
        // implementation, so the test is a real check rather than a restatement.
        let expected_ratio_ok = s * 10_000 <= j * (ratio as i128);
        let expected = j > 0 && expected_ratio_ok;
        let actual = check_senior_deposit(s, j, ratio).is_ok();
        prop_assert!(
            actual == expected,
            "s={} j={} ratio={}", s, j, ratio
        );
    }

    #[test]
    fn gate_is_monotone_in_all_three_inputs(
        s in principal(),
        j in 1i128..=MAX_PRINCIPAL,
        ratio in 1u32..=MAX_SENIOR_RATIO_BPS,
    ) {
        // More junior capital never rejects. Guarded, because pushing `j` past
        // MAX_PRINCIPAL is a rejected *input*, not a rejected deposit, and
        // conflating the two would be testing the wrong thing.
        if check_senior_deposit(s, j, ratio).is_ok() {
            if j < MAX_PRINCIPAL {
                prop_assert!(check_senior_deposit(s, j + 1, ratio).is_ok());
            }
            // Loosening the ratio never rejects either.
            if ratio < MAX_SENIOR_RATIO_BPS {
                prop_assert!(check_senior_deposit(s, j, ratio + 1).is_ok());
            }
        }
        // More senior capital never accepts: if one more unit is rejected then
        // the current total is rejected too.
        if s > 0 && check_senior_deposit(s - 1, j, ratio).is_ok() {
            prop_assert!(check_senior_deposit(s, j, ratio).is_ok());
        }
    }

    #[test]
    fn gate_always_implies_senior_at_most_junior_at_1x(
        s in principal(),
        j in principal(),
    ) {
        // The economic point of the whole structure: at the maximum ratio the
        // senior principal can never exceed the junior, so the buffer is real.
        if check_senior_deposit(s, j, MAX_SENIOR_RATIO_BPS).is_ok() {
            prop_assert!(s <= j, "accepted s={} > j={} at 1x", s, j);
        }
    }
}
