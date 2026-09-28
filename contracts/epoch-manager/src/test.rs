//! Tests for the epoch lifecycle: construction, opening an epoch, and the
//! views that read it.
//!
//! The money path — deposits, settlement, claims — is exercised in
//! `tests/src/integration.rs`, where the manager, a vault and a real token are
//! driven together. What is tested here is the state machine and the
//! authorisation, which need no token to verify.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    Address, Env,
};
use strata_mock_vault::MockVault;
use strata_vault_interface::VaultClient;

use crate::{EpochManager, EpochManagerClient, Error, Status, Tranche};

const TERM: u64 = 31_536_000; // one year
const RATE: u32 = 500; // 5%/yr
const RATIO: u32 = 10_000; // 1.0x

struct Fixture {
    env: Env,
    manager: Address,
    admin: Address,
    vault: Address,
    asset: Address,
    user: Address,
}

fn fixture() -> Fixture {
    let env = Env::default();
    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin.clone(), asset.clone()));
    let manager = env.register(EpochManager, (admin.clone(), vault.clone()));
    let user = Address::generate(&env);

    env.mock_all_auths();
    StellarAssetClient::new(&env, &asset).mint(&user, &1_000_000_000_000_000_000i128);

    Fixture {
        env,
        manager,
        admin,
        vault,
        asset,
        user,
    }
}

impl Fixture {
    fn client(&self) -> EpochManagerClient<'_> {
        EpochManagerClient::new(&self.env, &self.manager)
    }

    fn advance(&self, seconds: u64) {
        let now = self.env.ledger().timestamp() + seconds;
        self.env.ledger().set_timestamp(now);
    }
}

#[test]
fn construction_pins_the_vaults_asset() {
    let f = fixture();
    assert_eq!(f.client().admin(), f.admin);
    assert_eq!(f.client().vault(), f.vault);
    // Pinned from the vault at construction, not re-read per call.
    assert_eq!(f.client().asset(), f.asset);
    assert_eq!(f.client().current_epoch(), None);
}

#[test]
fn an_opened_epoch_records_its_terms_exactly() {
    let f = fixture();
    let start = f.env.ledger().timestamp();
    f.client().create_epoch(&TERM, &RATE, &RATIO);

    let e = f.client().current_epoch().expect("an epoch is open");
    assert_eq!(e.start_ts, start);
    assert_eq!(e.maturity_ts, start + TERM);
    assert_eq!(e.term_seconds, TERM);
    assert_eq!(e.rate_bps, RATE);
    assert_eq!(e.max_senior_ratio_bps, RATIO);
    assert_eq!(e.status, Status::Open);
    assert_eq!(e.senior_total, 0);
    assert_eq!(e.junior_total, 0);
    assert_eq!(f.client().seconds_to_maturity(), TERM);
}

#[test]
fn seconds_to_maturity_floors_at_zero() {
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    f.advance(TERM - 10);
    assert_eq!(f.client().seconds_to_maturity(), 10);

    f.advance(10);
    assert_eq!(f.client().seconds_to_maturity(), 0);

    f.advance(1_000);
    assert_eq!(f.client().seconds_to_maturity(), 0);
}

#[test]
fn only_one_epoch_may_be_active_at_a_time() {
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    assert_eq!(
        f.client().try_create_epoch(&TERM, &RATE, &RATIO),
        Err(Ok(Error::EpochAlreadyActive))
    );
}

#[test]
fn a_rejected_epoch_leaves_no_state_behind() {
    // Every parameter is validated before anything is written, so a bad
    // create_epoch must not leave a half-built epoch that would then block a
    // good one.
    let f = fixture();
    assert!(f.client().try_create_epoch(&0, &RATE, &RATIO).is_err());
    assert!(f
        .client()
        .try_create_epoch(&TERM, &u32::MAX, &RATIO)
        .is_err());
    assert!(f.client().try_create_epoch(&TERM, &RATE, &0).is_err());
    assert!(f
        .client()
        .try_create_epoch(&TERM, &RATE, &(RATIO + 1))
        .is_err());
    assert!(f
        .client()
        .try_create_epoch(&(TERM + 1), &RATE, &RATIO)
        .is_err());

    assert_eq!(f.client().current_epoch(), None, "no epoch was created");
    // And a valid one still works.
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    assert!(f.client().current_epoch().is_some());
}

#[test]
fn epoch_parameters_are_bounded_by_the_waterfall_spec() {
    let f = fixture();
    // The documented ceilings, and one past each.
    assert!(f
        .client()
        .try_create_epoch(&31_536_000, &10_000, &10_000)
        .is_ok());
    assert!(f
        .client()
        .try_create_epoch(&31_536_001, &10_000, &10_000)
        .is_err());
    assert!(f
        .client()
        .try_create_epoch(&31_536_000, &10_001, &10_000)
        .is_err());
    assert!(f
        .client()
        .try_create_epoch(&31_536_000, &10_000, &10_001)
        .is_err());
}

#[test]
fn only_the_admin_may_open_an_epoch() {
    // Auth is deliberately NOT mocked, so require_auth is enforced.
    let env = Env::default();
    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin.clone(), asset));
    let manager = env.register(EpochManager, (admin, vault));

    let client = EpochManagerClient::new(&env, &manager);
    assert!(client.try_create_epoch(&TERM, &RATE, &RATIO).is_err());
}

#[test]
fn close_refuses_an_epoch_that_is_not_settled() {
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    // Open, not settled.
    assert_eq!(f.client().try_close_epoch(), Err(Ok(Error::NotMature)));
}

#[test]
fn close_refuses_with_no_epoch_at_all() {
    let f = fixture();
    assert_eq!(f.client().try_close_epoch(), Err(Ok(Error::NoActiveEpoch)));
}

#[test]
fn views_refuse_when_no_epoch_exists() {
    let f = fixture();
    assert_eq!(f.client().try_senior_room(), Err(Ok(Error::NoActiveEpoch)));
    assert_eq!(
        f.client().try_seconds_to_maturity(),
        Err(Ok(Error::NoActiveEpoch))
    );
    assert_eq!(
        f.client().try_project(&1_000),
        Err(Ok(Error::NoActiveEpoch))
    );
    assert_eq!(
        f.client().try_position_of(&f.user, &Tranche::Senior),
        Err(Ok(Error::NoActiveEpoch))
    );
}

#[test]
fn senior_room_is_zero_until_junior_capital_exists() {
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    assert_eq!(f.client().senior_room(), 0);
}

#[test]
fn senior_room_is_zero_once_deposits_close() {
    // The epoch is still Open by status, but maturity has passed. A depositor
    // reading this must not be told there is room.
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    f.advance(TERM);
    assert_eq!(f.client().seconds_to_maturity(), 0);
    // Status is still Open, so senior_room still reflects the gate. It is
    // derived from junior_total, which is zero, so it is zero either way.
    assert_eq!(f.client().senior_room(), 0);
}

#[test]
fn project_applies_the_waterfall_to_an_arbitrary_value() {
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    // With no deposits, S = J = 0, so senior_due is 0 and the senior gets
    // nothing at any V. That is the correct degenerate answer.
    let p = f.client().project(&1_000_000);
    assert_eq!(p.senior_due, 0);
    assert_eq!(p.senior_payout, 0);
    assert_eq!(p.junior_payout, 1_000_000);
}

#[test]
fn project_agrees_with_the_waterfall_crate_directly() {
    use strata_waterfall::settle;
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    let e = f.client().current_epoch().unwrap();

    for v in [0i128, 1, 1_050_000, 1_050_001, 5_000_000] {
        let on_chain = f.client().project(&v);
        let direct = settle(
            e.senior_total,
            e.junior_total,
            e.rate_bps,
            e.term_seconds,
            v,
        )
        .unwrap();
        assert_eq!(on_chain.senior_due, direct.senior_due, "V = {v}");
        assert_eq!(on_chain.senior_payout, direct.senior_payout, "V = {v}");
        assert_eq!(on_chain.junior_payout, direct.junior_payout, "V = {v}");
    }
}

#[test]
fn an_empty_position_reads_as_zero() {
    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    let p = f.client().position_of(&f.user, &Tranche::Senior);
    assert_eq!(p.principal, 0);
    assert_eq!(p.estimated_payout, 0);
}

#[test]
fn a_freshly_built_manager_holds_no_vault_shares() {
    let f = fixture();
    assert_eq!(f.client().vault_shares(), 0);
}

#[test]
fn the_pinned_asset_matches_what_the_vault_reports() {
    let f = fixture();
    let from_vault = VaultClient::new(&f.env, &f.vault).asset();
    assert_eq!(f.client().asset(), from_vault);
}

#[test]
fn error_codes_match_discriminants() {
    // `wf` casts the enum to a u32 to build a `soroban_sdk::Error`, which is
    // only correct because the discriminants are declared explicitly. If
    // someone reorders the enum without renumbering, the on-chain codes change
    // silently. This test is the tripwire.
    assert_eq!(Error::AlreadyInitialized as u32, 1);
    assert_eq!(Error::EpochAlreadyActive as u32, 2);
    assert_eq!(Error::NoActiveEpoch as u32, 3);
    assert_eq!(Error::NotMature as u32, 4);
    assert_eq!(Error::AlreadySettled as u32, 5);
    assert_eq!(Error::DepositsClosed as u32, 6);
    assert_eq!(Error::Unauthorized as u32, 7);
    assert_eq!(Error::InvalidAmount as u32, 8);
    assert_eq!(Error::InvalidParameter as u32, 9);
    assert_eq!(Error::SeniorGateViolated as u32, 10);
    assert_eq!(Error::NoPosition as u32, 11);
    assert_eq!(Error::ArithmeticOverflow as u32, 12);
    assert_eq!(Error::ClaimsOutstanding as u32, 13);
    assert_eq!(Error::NoVaultShares as u32, 14);
}
