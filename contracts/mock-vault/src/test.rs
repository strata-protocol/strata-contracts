//! Tests for the mock vault's yield model.
//!
//! These are not Strata invariants. They exist so the integration tests can
//! trust the mock: if the mock's arithmetic is wrong, every cross-contract
//! result is wrong for the wrong reason.
//!
//! Failure cases go through the generated `try_` client methods rather than
//! hand-built argument vectors, so they stay in step with the contract's
//! signature.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Env,
};

use crate::{
    Error, MockVault, MockVaultClient, MAX_ABS_RATE_BPS, MAX_ACCRUAL_SECONDS, MAX_ASSETS,
    SECONDS_PER_YEAR,
};

/// Tokens minted to each test user up front: 1e25.
///
/// Large enough to cover [`MAX_ASSETS`] (1e24) so the ceiling tests can
/// actually reach the ceiling, and small enough that expected balances read as
/// "this minus that plus the change" without tripping compile-time overflow.
const INITIAL_BALANCE: i128 = 10_000_000_000_000_000_000_000_000;

/// Tokens pre-funded to the vault as a *strategy reserve*.
///
/// A real vault earns yield by holding something. The mock has no strategy, so
/// its yield is pure accounting: `total_assets` can grow to a figure the vault
/// has no tokens for, and redemption then fails for lack of balance. The
/// reserve supplies that backing, exactly as a strategy position would.
///
/// The reserve is a test artifact and is deliberately *not* counted in
/// `total_assets`, which stays the authoritative accounting figure the tests
/// assert on. What lands in the vault's real balance at the end of an epoch is
/// the reserve minus whatever was paid out.
const STRATEGY_RESERVE: i128 = 10_000_000_000_000_000_000_000_000;

// Rates in basis points, named by percentage. Basis points are easy to get
// wrong by a factor of ten -- 5000 bps is 50%, not 5% -- so every rate in
// these tests is written as one of these rather than as a bare number.
const PCT_5: i32 = 500;
const PCT_10: i32 = 1_000;
const PCT_100: i32 = 10_000;

struct Fixture {
    env: Env,
    vault: Address,
    asset: Address,
    admin: Address,
    user: Address,
}

fn fixture() -> Fixture {
    let env = Env::default();
    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin.clone(), asset.clone()));
    let user = Address::generate(&env);

    env.mock_all_auths();
    // Mint through the Stellar Asset Contract's own issuer interface, which
    // mock_all_auths satisfies.
    StellarAssetClient::new(&env, &asset).mint(&user, &INITIAL_BALANCE);
    StellarAssetClient::new(&env, &asset).mint(&vault, &STRATEGY_RESERVE);

    Fixture {
        env,
        vault,
        asset,
        admin,
        user,
    }
}

impl Fixture {
    fn client(&self) -> MockVaultClient<'_> {
        MockVaultClient::new(&self.env, &self.vault)
    }

    fn token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.env, &self.asset)
    }

    fn balance(&self, who: &Address) -> i128 {
        self.token().balance(who)
    }

    /// A second, funded depositor.
    fn other(&self) -> Address {
        let other = Address::generate(&self.env);
        StellarAssetClient::new(&self.env, &self.asset).mint(&other, &INITIAL_BALANCE);
        other
    }

    /// Moves the ledger clock forward by `seconds`.
    fn advance(&self, seconds: u64) {
        let now = self.env.ledger().timestamp() + seconds;
        self.env.ledger().set_timestamp(now);
    }

    fn deposit(&self, who: &Address, assets: i128) -> i128 {
        self.client().deposit(&assets, who)
    }
}

#[test]
fn starts_empty_with_a_zero_rate() {
    let f = fixture();
    assert_eq!(f.client().total_assets(), 0);
    assert_eq!(f.client().yield_rate(), 0);
    assert_eq!(f.client().share_balance(&f.user), 0);
    assert_eq!(f.client().underlying_asset(), f.asset);
    assert_eq!(f.client().admin(), f.admin);
}

#[test]
fn first_depositor_defines_the_price_at_one_to_one() {
    let f = fixture();
    let shares = f.deposit(&f.user, 1_000_000);
    assert_eq!(shares, 1_000_000);
    assert_eq!(f.client().total_assets(), 1_000_000);
    assert_eq!(f.client().convert_to_assets(&1_000_000), 1_000_000);
}

#[test]
fn a_flat_epoch_returns_the_principal() {
    let f = fixture();
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR);

    let assets = f.client().redeem(&1_000_000, &f.user, &f.user);
    assert_eq!(
        assets, 1_000_000,
        "a zero rate must return principal exactly"
    );
    assert_eq!(f.balance(&f.user), INITIAL_BALANCE);
}

/// The headline behaviour the brief asks for: an admin-configurable rate.
#[test]
fn a_good_epoch_compounds_upward() {
    let f = fixture();
    f.client().set_yield_rate(&PCT_5);
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR);

    let assets = f.client().redeem(&1_000_000, &f.user, &f.user);
    assert_eq!(assets, 1_050_000, "5% of 1_000_000 is 50_000");
    assert_eq!(f.balance(&f.user), INITIAL_BALANCE + 50_000);
}

/// And the same, negative.
#[test]
fn a_bad_epoch_loses_money() {
    let f = fixture();
    f.client().set_yield_rate(&(-PCT_5));
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR);

    let assets = f.client().redeem(&1_000_000, &f.user, &f.user);
    assert_eq!(assets, 950_000, "a -5% rate must cost 5% of principal");
    assert_eq!(f.balance(&f.user), INITIAL_BALANCE - 50_000);
}

#[test]
fn a_catastrophic_rate_zeroes_the_vault_without_going_negative() {
    let f = fixture();
    f.client().set_yield_rate(&(-MAX_ABS_RATE_BPS)); // -1000%/yr
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR);

    assert_eq!(
        f.client().total_assets(),
        0,
        "clamped at zero, not negative"
    );

    // Redeeming still works and returns nothing rather than failing.
    let assets = f.client().redeem(&1_000_000, &f.user, &f.user);
    assert_eq!(assets, 0);
    assert_eq!(
        f.client().share_balance(&f.user),
        0,
        "shares are still burned"
    );
}

#[test]
fn a_depositor_only_earns_for_the_time_they_were_held() {
    let f = fixture();
    f.client().set_yield_rate(&PCT_10); // 10%/yr

    // Half a year passes with nothing deposited, so nothing accrues.
    f.advance(SECONDS_PER_YEAR / 2);
    f.deposit(&f.user, 1_000_000);
    assert_eq!(
        f.client().total_assets(),
        1_000_000,
        "the base was zero for the first half year"
    );

    // The next half year earns 5%.
    f.advance(SECONDS_PER_YEAR / 2);
    assert_eq!(f.client().total_assets(), 1_050_000);
}

#[test]
fn a_redeemer_stops_earning_once_redeemed() {
    let f = fixture();
    f.client().set_yield_rate(&PCT_10);
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR / 2);
    assert_eq!(f.client().total_assets(), 1_050_000);

    let out = f.client().redeem(&1_000_000, &f.user, &f.user);
    assert_eq!(out, 1_050_000);
    assert_eq!(f.client().total_assets(), 0, "empty after a full exit");

    // More time earns nothing, because the base is zero.
    f.advance(SECONDS_PER_YEAR);
    assert_eq!(f.client().total_assets(), 0);
}

#[test]
fn changing_the_rate_is_not_retroactive() {
    let f = fixture();
    f.deposit(&f.user, 1_000_000);

    f.advance(SECONDS_PER_YEAR / 2);
    // The first half year ran at the default 0%, and setting the rate
    // accrues first, so it must not retroactively reprice that window.
    f.client().set_yield_rate(&PCT_10);
    assert_eq!(f.client().total_assets(), 1_000_000);

    f.advance(SECONDS_PER_YEAR / 2);
    assert_eq!(f.client().total_assets(), 1_050_000);
}

#[test]
fn accrual_compounds_across_windows() {
    let f = fixture();
    f.client().set_yield_rate(&PCT_10);
    f.deposit(&f.user, 1_000_000);

    // Four quarters at 2.5% each, compounding: 1_000_000 * 1.025^4.
    for _ in 0..4 {
        f.advance(SECONDS_PER_YEAR / 4);
        f.client().accrue();
    }
    let total = f.client().total_assets();
    // Simple interest over the year would be exactly 1_100_000; compounding
    // gives a little more, and 1.025^4 is about 1_103_813.
    assert!(total > 1_100_000, "expected compounding, got {total}");
    assert!(total < 1_110_000, "expected roughly 1.025^4, got {total}");
}

#[test]
fn later_depositors_do_not_dilute_earlier_ones() {
    let f = fixture();
    f.client().set_yield_rate(&MAX_ABS_RATE_BPS); // 1000%/yr
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR);
    f.client().accrue();

    // Price has moved, so the same assets buy fewer shares.
    let first_share_price = f.client().convert_to_assets(&1_000_000);
    assert!(
        first_share_price > 1_000_000,
        "price moved to {first_share_price}"
    );

    let other = f.other();
    let shares = f.deposit(&other, 1_000_000);
    assert!(shares < 1_000_000, "a later depositor gets fewer shares");
    // 4626 rounds both directions, so the round trip can land a base unit
    // under. That is expected and is not a bug; the point is that it is close
    // to, not far from, the value paid in.
    let value_back = f.client().convert_to_assets(&shares);
    assert!(
        (1_000_000 - value_back).abs() <= 1,
        "expected 1_000_000 back, got {value_back}"
    );
}

#[test]
fn multiple_depositors_split_pro_rata() {
    let f = fixture();
    f.client().set_yield_rate(&PCT_10);
    let other = f.other();

    f.deposit(&f.user, 3_000_000);
    f.deposit(&other, 1_000_000);
    f.advance(SECONDS_PER_YEAR);

    // 10% of 4_000_000 is 400_000, so 4_400_000 to split 3:1.
    let mine = f.client().redeem(&3_000_000, &f.user, &f.user);
    let theirs = f.client().redeem(&1_000_000, &other, &other);
    assert_eq!(mine, 3_300_000);
    assert_eq!(theirs, 1_100_000);
    assert_eq!(mine + theirs, 4_400_000);
}

#[test]
fn a_deposit_into_a_wiped_vault_is_refused_rather_than_issued_for_nothing() {
    let f = fixture();
    f.client().set_yield_rate(&(-MAX_ABS_RATE_BPS));
    f.deposit(&f.user, 1_000_000);
    f.advance(SECONDS_PER_YEAR);
    assert_eq!(f.client().total_assets(), 0);

    let other = f.other();
    // A wiped vault has a zero share price, so 1_000_000 assets would mint
    // zero shares. Refusing beats taking the assets and issuing nothing.
    assert!(f.client().try_deposit(&1_000_000, &other).is_err());
    assert_eq!(f.balance(&other), INITIAL_BALANCE, "no assets were taken");
}

#[test]
fn cannot_redeem_more_shares_than_held() {
    let f = fixture();
    f.deposit(&f.user, 1_000_000);
    assert!(f.client().try_redeem(&2_000_000, &f.user, &f.user).is_err());
}

#[test]
fn a_rate_beyond_the_documented_bound_is_refused() {
    let f = fixture();
    assert!(f
        .client()
        .try_set_yield_rate(&(MAX_ABS_RATE_BPS + 1))
        .is_err());
    assert_eq!(f.client().yield_rate(), 0, "the rate was not changed");
}

#[test]
fn only_the_admin_may_set_the_rate() {
    // Auth is deliberately NOT mocked, so require_auth is actually enforced.
    let env = Env::default();
    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin, asset));

    let client = MockVaultClient::new(&env, &vault);
    assert!(
        client.try_set_yield_rate(&PCT_5).is_err(),
        "an unauthorised rate change must fail"
    );
}

#[test]
fn the_asset_ceiling_keeps_accrual_inside_i128() {
    // The vault's whole overflow argument rests on this product. A stray digit
    // group in MAX_ASSETS silently invalidates it -- an earlier draft had
    // 1e27, which overflows -- so the arithmetic is pinned here rather than
    // left as a claim in a doc comment.
    let widest = MAX_ASSETS
        .checked_mul(MAX_ABS_RATE_BPS as i128)
        .and_then(|p| p.checked_mul(MAX_ACCRUAL_SECONDS as i128))
        .expect("the documented bound must not overflow on its own");

    assert!(
        widest < i128::MAX,
        "widest accrual product {widest} must stay under i128::MAX {}",
        i128::MAX
    );
    // And with room to spare, so a future rate or window bump fails loudly.
    assert!(
        widest * 2 < i128::MAX,
        "expected roughly 5x headroom, got {}x",
        i128::MAX / widest
    );
    assert_eq!(MAX_ASSETS, 1_000_000_000_000_000_000_000_000, "1e24");
}

#[test]
fn total_assets_is_capped_so_the_accrual_product_cannot_overflow() {
    let f = fixture();
    f.deposit(&f.user, MAX_ASSETS);
    f.client().set_yield_rate(&MAX_ABS_RATE_BPS);

    // The maximum rate over the maximum accrual window. Without the ceiling
    // this product would be 1e24 * 1e5 * 3.15e8 and would wrap i128.
    f.advance(MAX_ACCRUAL_SECONDS);
    assert_eq!(f.client().total_assets(), MAX_ASSETS);
}

#[test]
fn depositing_past_the_asset_ceiling_is_refused_rather_than_truncating() {
    let f = fixture();
    f.deposit(&f.user, MAX_ASSETS);
    assert!(f.client().try_deposit(&1_000_000, &f.user).is_err());
    assert_eq!(f.client().total_assets(), MAX_ASSETS, "state unchanged");
}

#[test]
fn accrual_stops_rather_than_re_accruing_a_clamped_window() {
    let f = fixture();
    f.client().set_yield_rate(&PCT_100);
    f.deposit(&f.user, 1_000_000);

    // Far beyond the accrual window. The vault accrues at most
    // MAX_ACCRUAL_SECONDS and drops the remainder, and calling accrue again
    // must not re-accrue that dropped tail.
    f.advance(100 * SECONDS_PER_YEAR);
    let once = f.client().total_assets();
    f.client().accrue();
    assert_eq!(f.client().total_assets(), once);
}

#[test]
fn the_error_enum_is_reachable_for_diagnostics() {
    // Not a behaviour test: keeps variants from being flagged as
    // never-constructed, which would mean a typo in a panic message.
    assert_eq!(Error::ZeroAmount, Error::ZeroAmount);
    assert!(Error::ZeroShares < Error::InsufficientShares);
    assert!(Error::ArithmeticOverflow < Error::RateOutOfRange);
    assert!(Error::AssetCeilingReached < Error::RateOutOfRange);
    assert!(Error::Unauthorized < Error::AlreadyInitialized);
}
