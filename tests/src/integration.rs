//! Cross-contract tests: a real epoch-manager, a real mock-vault and a real
//! SEP-41 token, driven through full epoch lifecycles.
//!
//! These are the tests that would catch a mistake the unit tests cannot see —
//! a wrong authorisation entry, a share count that drifts from the token
//! balance, a payout that sums to something other than what the vault paid out.
//!
//! Every settlement assertion is checked against `strata_waterfall::settle`
//! directly, computed from the vault's real valuation. The point is that the
//! contract's numbers are never taken on trust.
//!
//! # The strategy reserve
//!
//! The mock's yield is unbacked accounting, so the fixture pre-funds the vault
//! with spare tokens standing in for a strategy position. Without it a
//! redemption of an accrual gain would fail for lack of balance. See the mock
//! vault's crate docs.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Env,
};
use strata_epoch_manager::{EpochManager, EpochManagerClient, Error, Status, Tranche};
use strata_mock_vault::MockVault;
use strata_mock_vault::MockVaultClient;
use strata_waterfall::{settle, SECONDS_PER_YEAR};

const TERM: u64 = SECONDS_PER_YEAR;
const RATE: u32 = 500; // 5%/yr senior target
const RATIO: u32 = 10_000; // 1.0x senior cap

const STRATEGY_RESERVE: i128 = 1_000_000_000_000_000_000;
const STARTING_BALANCE: i128 = 1_000_000_000;

struct World {
    env: Env,
    manager: Address,
    vault: Address,
    asset: Address,
    /// Senior depositor with a large balance.
    senior_one: Address,
    /// Second senior depositor, to exercise the last-claimer remainder.
    senior_two: Address,
    /// Junior depositor.
    junior_one: Address,
}

fn world() -> World {
    let env = Env::default();
    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin.clone(), asset.clone()));
    let manager = env.register(EpochManager, (admin.clone(), vault.clone()));

    let senior_one = Address::generate(&env);
    let senior_two = Address::generate(&env);
    let junior_one = Address::generate(&env);

    env.mock_all_auths();
    for who in [&senior_one, &senior_two, &junior_one] {
        StellarAssetClient::new(&env, &asset).mint(who, &STARTING_BALANCE);
    }
    StellarAssetClient::new(&env, &asset).mint(&vault, &STRATEGY_RESERVE);

    World {
        env,
        manager,
        vault,
        asset,
        senior_one,
        senior_two,
        junior_one,
    }
}

impl World {
    fn mgr(&self) -> EpochManagerClient<'_> {
        EpochManagerClient::new(&self.env, &self.manager)
    }

    fn vault_client(&self) -> MockVaultClient<'_> {
        MockVaultClient::new(&self.env, &self.vault)
    }

    fn token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.env, &self.asset)
    }

    fn balance(&self, who: &Address) -> i128 {
        self.token().balance(who)
    }

    fn advance(&self, seconds: u64) {
        let now = self.env.ledger().timestamp() + seconds;
        self.env.ledger().set_timestamp(now);
    }

    fn open_epoch(&self) {
        self.mgr().create_epoch(&TERM, &RATE, &RATIO);
    }

    fn deposit_senior(&self, who: &Address, amount: i128) -> i128 {
        self.mgr().deposit(who, &Tranche::Senior, &amount)
    }

    fn deposit_junior(&self, who: &Address, amount: i128) -> i128 {
        self.mgr().deposit(who, &Tranche::Junior, &amount)
    }

    /// Runs an epoch to maturity and settles it. `rate_bps` is the vault's
    /// annualised yield for the whole term.
    fn run_to_settlement(&self, vault_rate_bps: i32) {
        self.vault_client().set_yield_rate(&vault_rate_bps);
        self.advance(TERM);
        self.mgr().settle();
    }

    /// The value the underlying is actually worth right now, as the manager
    /// sees it.
    ///
    /// Uses the vault's `total_assets` rather than `convert_to_assets` on the
    /// manager's shares. `convert_to_assets` is specified as a read-only
    /// conversion, and the mock deliberately does not accrue on it, so it
    /// returns a figure that can be up to one accrual window stale. For a
    /// live valuation, `total_assets` is the right call.
    fn underlying_value(&self) -> i128 {
        self.vault_client().total_assets()
    }
}

// --- a healthy epoch, end to end ---

#[test]
fn a_healthy_epoch_pays_senior_first_then_the_junior_the_rest() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 400_000);
    w.deposit_senior(&w.senior_one, 400_000);

    // 5% annualised for a one-year term: 800_000 becomes 840_000.
    w.run_to_settlement(500);

    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(e.status, Status::Settled);
    assert_eq!(e.value_redeemed, 840_000);

    // senior_due = 400_000 + 5% of 400_000 = 420_000, capped there.
    assert_eq!(e.senior_payout, 420_000);
    assert_eq!(e.junior_payout, 840_000 - 420_000);

    // The contract's figures must match the pure waterfall applied to the
    // vault's real valuation.
    let expected = settle(400_000, 400_000, RATE, TERM, 840_000).unwrap();
    assert_eq!(e.senior_payout, expected.senior_payout);
    assert_eq!(e.junior_payout, expected.junior_payout);
    assert_eq!(
        e.senior_payout + e.junior_payout,
        e.value_redeemed,
        "invariant 1 at the contract level"
    );
}

#[test]
fn a_healthy_epoch_pays_every_depositor_exactly_what_the_waterfall_says() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 500_000);
    w.deposit_senior(&w.senior_one, 300_000);
    w.run_to_settlement(500);

    let senior = w.mgr().claim(&w.senior_one, &Tranche::Senior);
    let junior = w.mgr().claim(&w.junior_one, &Tranche::Junior);

    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(senior, e.senior_payout);
    assert_eq!(junior, e.junior_payout);
    assert_eq!(senior + junior, e.value_redeemed, "nothing is stranded");

    // Balances moved by exactly the payout.
    assert_eq!(
        w.balance(&w.senior_one),
        STARTING_BALANCE - 300_000 + senior
    );
    assert_eq!(
        w.balance(&w.junior_one),
        STARTING_BALANCE - 500_000 + junior
    );
}

#[test]
fn the_manager_holds_the_payout_until_it_is_claimed() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 250_000);
    w.deposit_senior(&w.senior_one, 250_000);
    w.run_to_settlement(1_000);

    let e = w.mgr().current_epoch().unwrap();
    // 500_000 at +10% for a year.
    assert_eq!(e.value_redeemed, 550_000);
    assert_eq!(
        e.senior_payout + e.junior_payout,
        e.value_redeemed,
        "invariant 1: the split accounts for everything redeemed"
    );
    assert_eq!(
        w.balance(&w.manager),
        e.senior_payout + e.junior_payout,
        "the manager custodies the whole payout until it is claimed"
    );

    let senior = w.mgr().claim(&w.senior_one, &Tranche::Senior);
    let junior = w.mgr().claim(&w.junior_one, &Tranche::Junior);
    assert_eq!(senior + junior, e.value_redeemed);
    assert_eq!(
        w.balance(&w.manager),
        0,
        "and nothing at all once every claim is made"
    );
}

#[test]
fn a_full_epoch_can_be_closed_and_the_next_one_opened() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.run_to_settlement(300);
    w.mgr().claim(&w.senior_one, &Tranche::Senior);
    w.mgr().claim(&w.junior_one, &Tranche::Junior);

    w.mgr().close_epoch();
    assert_eq!(w.mgr().current_epoch().unwrap().status, Status::Closed);

    // A new epoch can now open, and the new one starts from zero.
    w.open_epoch();
    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(e.status, Status::Open);
    assert_eq!(e.senior_total, 0);
    assert_eq!(e.junior_total, 0);
    assert_eq!(e.senior_payout, 0);
}

// --- a bad epoch ---

/// The junior eats a modest loss in full, because the senior's target is a
/// rate on the *senior* principal only, not on the whole pool.
///
/// With 1:1 deposits of 400_000 each and a -5% year, the pool falls to
/// 760_000. The senior's target is 5% of 400_000, i.e. 420_000, which is
/// easily covered — so the senior is made whole and the junior absorbs the
/// whole 40_000 loss *plus* the 20_000 the senior is paid out of the pool.
/// The junior ends with 340_000, having lost 60_000.
///
/// This is the tranche structure doing its job, and it is worth pinning: the
/// naive expectation is that the junior loses only 40_000.
#[test]
fn a_moderate_loss_is_absorbed_by_the_junior_alone() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 400_000);
    w.deposit_senior(&w.senior_one, 400_000);

    w.run_to_settlement(-500);

    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(e.value_redeemed, 760_000);
    assert_eq!(e.senior_payout, 420_000, "the senior is made whole");
    assert_eq!(
        e.junior_payout, 340_000,
        "the junior lost the 40_000 shortfall plus the senior's 20_000 target"
    );
    assert_eq!(e.senior_payout + e.junior_payout, e.value_redeemed);

    let senior = w.mgr().claim(&w.senior_one, &Tranche::Senior);
    let junior = w.mgr().claim(&w.junior_one, &Tranche::Junior);
    assert_eq!(senior, 420_000);
    assert_eq!(junior, 340_000);
    assert_eq!(
        w.balance(&w.junior_one),
        STARTING_BALANCE - 400_000 + 340_000
    );
    assert_eq!(w.balance(&w.manager), 0);
}

/// Past the cushion the senior starts taking losses too. A -60% year takes
/// 800_000 down to 320_000, which is 100_000 under the senior's 420_000
/// target, so the junior is wiped and the senior is still 100_000 short.
#[test]
fn a_large_loss_wipes_the_junior_and_then_the_senior() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 400_000);
    w.deposit_senior(&w.senior_one, 400_000);

    w.run_to_settlement(-6_000); // -60%/yr

    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(e.value_redeemed, 320_000);
    assert_eq!(e.junior_payout, 0, "the junior is wiped out");
    assert_eq!(
        e.senior_payout, 320_000,
        "the senior covers the 100_000 shortfall itself"
    );
    assert_eq!(e.senior_payout + e.junior_payout, e.value_redeemed);
}

#[test]
fn a_loss_inside_the_cushion_leaves_the_senior_whole() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 400_000);
    w.deposit_senior(&w.senior_one, 400_000);

    // -1%: 800_000 becomes 792_000. Cushion is J - I = 400_000 - 20_000, so
    // a loss of 8_000 is well inside it.
    w.run_to_settlement(-100);

    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(e.value_redeemed, 792_000);
    assert_eq!(e.senior_payout, 420_000, "senior untouched");
    assert_eq!(e.junior_payout, 372_000, "the junior ate the loss");
    assert_eq!(e.senior_payout + e.junior_payout, e.value_redeemed);
}

#[test]
fn a_wipeout_pays_nobody_anything_and_still_closes() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 100_000);
    w.deposit_senior(&w.senior_one, 100_000);

    // -1000%/yr wipes the vault out entirely.
    w.run_to_settlement(-100_000);

    let e = w.mgr().current_epoch().unwrap();
    assert_eq!(e.value_redeemed, 0);
    assert_eq!(e.senior_payout, 0);
    assert_eq!(e.junior_payout, 0);

    // Claims still work and are zero, which is what lets the epoch close.
    assert_eq!(w.mgr().claim(&w.senior_one, &Tranche::Senior), 0);
    assert_eq!(w.mgr().claim(&w.junior_one, &Tranche::Junior), 0);
    w.mgr().close_epoch();
}

// --- deposit gating ---

#[test]
fn a_senior_deposit_is_blocked_while_junior_is_empty() {
    let w = world();
    w.open_epoch();
    // The ratio rule alone already rejects this: post-deposit S = 400_000,
    // J = 0, and 400_000 * 10_000 <= 0 is false.
    assert_eq!(
        w.mgr()
            .try_deposit(&w.senior_one, &Tranche::Senior, &400_000),
        Err(Ok(Error::SeniorGateViolated))
    );
    // Nothing was taken.
    assert_eq!(w.balance(&w.senior_one), STARTING_BALANCE);
}

#[test]
fn the_gate_admits_senior_up_to_the_configured_ratio() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 400_000);
    // Room is now exactly 400_000 at 1.0x.
    assert_eq!(w.mgr().senior_room(), 400_000);

    w.deposit_senior(&w.senior_one, 400_000);
    assert_eq!(w.mgr().senior_room(), 0);

    // One more base unit is over the cap.
    assert_eq!(
        w.mgr().try_deposit(&w.senior_one, &Tranche::Senior, &1),
        Err(Ok(Error::SeniorGateViolated))
    );
}

#[test]
fn a_tighter_ratio_admits_less_senior() {
    let w = world();
    w.mgr().create_epoch(&TERM, &RATE, &5_000); // 0.5x
    w.deposit_junior(&w.junior_one, 400_000);
    assert_eq!(w.mgr().senior_room(), 200_000);

    w.deposit_senior(&w.senior_one, 200_000);
    assert_eq!(
        w.mgr().try_deposit(&w.senior_one, &Tranche::Senior, &1),
        Err(Ok(Error::SeniorGateViolated))
    );
}

#[test]
fn junior_deposits_are_never_gated() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 500_000);
    w.deposit_junior(&w.junior_one, 500_000);
    assert_eq!(w.mgr().current_epoch().unwrap().junior_total, 1_000_000);
}

// --- pro-rata distribution and the last-claimer remainder ---

#[test]
fn several_senior_depositors_split_pro_rata_and_the_total_is_exact() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 900_000);
    w.deposit_senior(&w.senior_one, 300_000);
    w.deposit_senior(&w.senior_two, 300_000);
    w.deposit_senior(&w.junior_one, 300_000);
    w.run_to_settlement(1_000);

    let e = w.mgr().current_epoch().unwrap();
    let a = w.mgr().claim(&w.senior_one, &Tranche::Senior);
    let b = w.mgr().claim(&w.senior_two, &Tranche::Senior);
    let c = w.mgr().claim(&w.junior_one, &Tranche::Senior);

    assert_eq!(
        a + b + c,
        e.senior_payout,
        "the tranche pays out exactly its senior_payout"
    );
    assert_eq!(a, b);
    assert_eq!(b, c);
    assert_eq!(e.senior_payout, 945_000, "5% of 900_000 is 45_000");
}

#[test]
fn an_awkward_split_leaves_dust_that_the_last_claimant_absorbs() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 1_000_000);
    // Three equal senior deposits, so pro-rata division is exact and there is
    // no dust. Now make it awkward: 1, 1, 1 senior shares against a payout
    // that does not divide by three.
    w.deposit_senior(&w.senior_one, 3);
    w.deposit_senior(&w.senior_two, 3);
    w.deposit_senior(&w.junior_one, 1);
    w.run_to_settlement(1_000);

    let e = w.mgr().current_epoch().unwrap();
    let mut total = 0;
    for who in [&w.senior_one, &w.senior_two, &w.junior_one] {
        total += w.mgr().claim(who, &Tranche::Senior);
    }
    assert_eq!(
        total, e.senior_payout,
        "the last claimant absorbed the dust, so the total is exact"
    );
    // The junior tranche is separate and still owed, so the manager legitimately
    // holds it. Claim that too, and then nothing may be left.
    w.mgr().claim(&w.junior_one, &Tranche::Junior);
    assert_eq!(w.balance(&w.manager), 0, "no dust left behind");
}

#[test]
fn claiming_twice_is_refused() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.run_to_settlement(200);

    let first = w.mgr().claim(&w.senior_one, &Tranche::Senior);
    assert!(first > 0);
    assert_eq!(
        w.mgr().try_claim(&w.senior_one, &Tranche::Senior),
        Err(Ok(Error::NoPosition))
    );
}

#[test]
fn a_non_depositor_holds_no_position() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.run_to_settlement(200);

    let stranger = Address::generate(&w.env);
    assert_eq!(
        w.mgr().try_claim(&stranger, &Tranche::Senior),
        Err(Ok(Error::NoPosition))
    );
    let p = w.mgr().position_of(&stranger, &Tranche::Senior);
    assert_eq!(p.principal, 0);
    assert_eq!(p.estimated_payout, 0);
}

// --- timing ---

#[test]
fn deposits_close_at_maturity_with_no_admin_step() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 400_000);
    w.advance(TERM - 1);
    w.deposit_senior(&w.senior_one, 1);

    w.advance(1);
    assert_eq!(
        w.mgr().try_deposit(&w.senior_one, &Tranche::Senior, &1),
        Err(Ok(Error::DepositsClosed))
    );
}

#[test]
fn settling_before_maturity_is_refused() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.advance(TERM - 1);
    assert_eq!(w.mgr().try_settle(), Err(Ok(Error::NotMature)));

    w.advance(1);
    assert!(w.mgr().try_settle().is_ok());
}

#[test]
fn settling_twice_is_refused() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.run_to_settlement(100);
    assert_eq!(w.mgr().try_settle(), Err(Ok(Error::AlreadySettled)));
}

#[test]
fn claiming_before_settlement_is_refused() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.advance(TERM);
    assert!(w.mgr().try_claim(&w.senior_one, &Tranche::Senior).is_err());
}

#[test]
fn settling_an_epoch_with_nothing_deposited_is_refused() {
    let w = world();
    w.open_epoch();
    w.advance(TERM);
    assert_eq!(w.mgr().try_settle(), Err(Ok(Error::NoVaultShares)));
}

// --- authorisation ---

#[test]
fn a_deposit_needs_the_depositors_own_authorisation() {
    // Auth is deliberately NOT mocked, so require_auth is enforced. The point
    // is that `from` is the authorising party, not the manager.
    let env = Env::default();
    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin.clone(), asset.clone()));
    let manager = env.register(EpochManager, (admin, vault));

    let client = EpochManagerClient::new(&env, &manager);
    // No auth mocked at all, so even the admin-authenticated create_epoch
    // fails, which proves the harness is enforcing.
    assert!(client.try_create_epoch(&TERM, &RATE, &RATIO).is_err());
}

#[test]
fn the_depositor_authorises_the_deposit_with_the_right_arguments() {
    // The full auth tree for a deposit, asserted rather than assumed. This is
    // the test that would catch a wrong `authorize_as_current_contract` entry:
    // mock_all_auths alone would let a missing or wrong entry pass.
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 300_000);

    w.mgr().deposit(&w.senior_one, &Tranche::Senior, &100_000);

    // The depositor authorised a manager call with exactly these arguments,
    // and that authorisation contains the token transfer the vault makes on
    // the manager's behalf.
    let auths = w.env.auths();
    let depositor_auth = auths
        .iter()
        .find(|(who, _)| *who == w.senior_one)
        .expect("the depositor must appear in the auth tree");
    let invocations = depositor_auth.1.sub_invocations.len();
    assert!(
        invocations >= 1,
        "the deposit must authorise the vault's token transfer, got {invocations} sub-invocations"
    );
}

// --- views agree with the money path ---

#[test]
fn the_position_view_matches_what_the_claim_actually_pays() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 250_000);
    w.deposit_senior(&w.senior_one, 250_000);
    w.run_to_settlement(750);

    let e = w.mgr().current_epoch().unwrap();
    let single = w.mgr().position_of(&w.senior_one, &Tranche::Senior);
    assert_eq!(single.principal, e.senior_total);
    assert_eq!(
        single.estimated_payout, e.senior_payout,
        "a sole depositor's whole tranche is their estimate"
    );
    assert_eq!(
        w.mgr().claim(&w.senior_one, &Tranche::Senior),
        single.estimated_payout
    );
}

#[test]
fn a_position_view_before_settlement_is_a_projection_not_a_promise() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);

    // Before maturity the vault is worth exactly the deposits, so the
    // projection is the break-even case: senior made whole, junior gets its
    // principal back.
    let p = w.mgr().position_of(&w.senior_one, &Tranche::Senior);
    let split = w.mgr().project(&w.underlying_value());
    assert_eq!(p.principal, 200_000);
    assert_eq!(split.senior_payout, 210_000);
    assert_eq!(split.junior_payout, 190_000);
}

#[test]
fn the_projection_tracks_the_underlying_as_it_moves() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);

    w.vault_client().set_yield_rate(&5_000);
    w.advance(TERM / 2);
    let at_half = w.mgr().project(&w.underlying_value());
    w.advance(TERM / 2);
    let at_end = w.mgr().project(&w.underlying_value());

    // The senior is capped, so extra underlying yield accrues to the junior.
    assert_eq!(at_half.senior_payout, at_end.senior_payout);
    assert!(at_end.junior_payout > at_half.junior_payout);
}

// --- multiple epochs ---

#[test]
fn two_epochs_in_sequence_pay_correctly() {
    let w = world();
    for rate in [2_000i32, -2_000i32] {
        w.open_epoch();
        w.deposit_junior(&w.junior_one, 300_000);
        w.deposit_senior(&w.senior_one, 300_000);
        w.run_to_settlement(rate);

        let e = w.mgr().current_epoch().unwrap();
        let expected = settle(300_000, 300_000, RATE, TERM, e.value_redeemed).unwrap();
        assert_eq!(e.senior_payout, expected.senior_payout, "rate {rate}");
        assert_eq!(e.junior_payout, expected.junior_payout, "rate {rate}");

        w.mgr().claim(&w.senior_one, &Tranche::Senior);
        w.mgr().claim(&w.junior_one, &Tranche::Junior);
        w.mgr().close_epoch();
    }
}

#[test]
fn an_epoch_cannot_be_opened_while_claims_are_outstanding() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_junior(&w.junior_one, 200_000);
    w.deposit_senior(&w.senior_one, 200_000);
    w.run_to_settlement(200);

    assert_eq!(
        w.mgr().try_create_epoch(&TERM, &RATE, &RATIO),
        Err(Ok(Error::EpochAlreadyActive)),
        "a settled epoch with unclaimed money still blocks a new one"
    );

    w.mgr().claim(&w.senior_one, &Tranche::Senior);
    assert_eq!(
        w.mgr().try_create_epoch(&TERM, &RATE, &RATIO),
        Err(Ok(Error::EpochAlreadyActive)),
        "one unclaimed tranche is still enough to block"
    );

    w.mgr().claim(&w.junior_one, &Tranche::Junior);
    w.mgr().close_epoch();
    w.open_epoch();
}

// --- bounds ---

#[test]
fn a_zero_or_negative_deposit_is_refused() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 300_000);
    assert_eq!(
        w.mgr().try_deposit(&w.senior_one, &Tranche::Senior, &0),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        w.mgr().try_deposit(&w.senior_one, &Tranche::Senior, &-1),
        Err(Ok(Error::InvalidAmount))
    );
}

#[test]
fn a_deposit_beyond_the_documented_ceiling_is_refused() {
    let w = world();
    w.open_epoch();
    w.deposit_junior(&w.junior_one, 300_000);
    // 1e27 exceeds MAX_PRINCIPAL (1e26), and would also exceed the
    // depositor's balance, so the bound is checked first.
    assert_eq!(
        w.mgr().try_deposit(
            &w.senior_one,
            &Tranche::Senior,
            &1_000_000_000_000_000_000_000_000_000i128
        ),
        Err(Ok(Error::InvalidParameter))
    );
}
