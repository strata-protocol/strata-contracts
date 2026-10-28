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

#[test]
fn storage_entries_record_expected_initial_ttls_across_manager_and_vault() {
    use soroban_sdk::testutils::storage::{Instance as _, Persistent as _};
    use strata_mock_vault::DataKey as VaultKey;

    use crate::{DataKey, BUMP_AMOUNT};

    let f = fixture();
    let min_persistent_ttl = f.env.ledger().get().min_persistent_entry_ttl - 1;

    // Constructor writes Config and bumps it to BUMP_AMOUNT; instance storage
    // is not explicitly bumped in contract code and starts at the network
    // minimum persistent TTL.
    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&DataKey::Config),
            BUMP_AMOUNT
        );
        assert_eq!(f.env.storage().instance().get_ttl(), min_persistent_ttl);
    });

    f.client().create_epoch(&TERM, &RATE, &RATIO);
    f.client().deposit(&f.user, &Tranche::Junior, &1_000_000);
    f.client().deposit(&f.user, &Tranche::Senior, &1_000_000);

    let junior_key = DataKey::Position(f.user.clone(), Tranche::Junior);
    let senior_key = DataKey::Position(f.user.clone(), Tranche::Senior);

    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&DataKey::Epoch),
            BUMP_AMOUNT
        );
        assert_eq!(
            f.env.storage().persistent().get_ttl(&junior_key),
            BUMP_AMOUNT
        );
        assert_eq!(
            f.env.storage().persistent().get_ttl(&senior_key),
            BUMP_AMOUNT
        );
    });

    // mock-vault writes persistent entries without calling extend_ttl, so each
    // entry starts at `min_persistent_entry_ttl - 1` (4_095 ledgers in default
    // test LedgerInfo).
    f.env.as_contract(&f.vault, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&VaultKey::Admin),
            min_persistent_ttl
        );
        assert_eq!(
            f.env.storage().persistent().get_ttl(&VaultKey::TotalAssets),
            min_persistent_ttl
        );
        assert_eq!(
            f.env
                .storage()
                .persistent()
                .get_ttl(&VaultKey::ShareBalance(f.manager.clone())),
            min_persistent_ttl
        );
    });
}

#[test]
fn position_ttl_only_extends_once_remaining_life_reaches_bump_threshold() {
    use soroban_sdk::testutils::storage::Persistent as _;

    use crate::{DataKey, BUMP_AMOUNT, BUMP_THRESHOLD};

    let f = fixture();
    f.client().create_epoch(&TERM, &RATE, &RATIO);
    f.client().deposit(&f.user, &Tranche::Junior, &1_000_000);

    let junior_key = DataKey::Position(f.user.clone(), Tranche::Junior);

    // Advance so remaining TTL is BUMP_THRESHOLD + 1 (100_001 ledgers).
    // `extend_ttl` only extends when `current_ttl <= BUMP_THRESHOLD`, so a
    // deposit here leaves the remaining TTL at 100_001.
    let start_seq = f.env.ledger().sequence();
    f.env
        .ledger()
        .set_sequence_number(start_seq + BUMP_AMOUNT - BUMP_THRESHOLD - 1);

    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&junior_key),
            BUMP_THRESHOLD + 1
        );
    });

    f.client().deposit(&f.user, &Tranche::Junior, &500_000);
    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&junior_key),
            BUMP_THRESHOLD + 1
        );
    });

    // Advance one more ledger so remaining TTL == BUMP_THRESHOLD (100_000).
    // Touching the position now bumps its TTL back to BUMP_AMOUNT (4_000_000).
    f.env
        .ledger()
        .set_sequence_number(start_seq + BUMP_AMOUNT - BUMP_THRESHOLD);

    f.client().deposit(&f.user, &Tranche::Junior, &500_000);
    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&junior_key),
            BUMP_AMOUNT
        );
    });
}

#[test]
fn position_archived_past_bump_amount_in_max_term_epoch_is_restored_and_claimable() {
    use soroban_sdk::testutils::storage::Persistent as _;
    use soroban_sdk::token::TokenClient;

    use crate::{DataKey, BUMP_AMOUNT};

    let f = fixture();
    // Mint strategy reserve to the vault so a 1-year positive-yield settlement
    // can pay out.
    StellarAssetClient::new(&f.env, &f.asset).mint(&f.vault, &1_000_000_000_000i128);

    f.client().create_epoch(&TERM, &RATE, &RATIO);
    f.client().deposit(&f.user, &Tranche::Junior, &100_000_000);
    f.client().deposit(&f.user, &Tranche::Senior, &100_000_000);

    let senior_key = DataKey::Position(f.user.clone(), Tranche::Senior);
    let start_seq = f.env.ledger().sequence();

    // At exactly `start_seq + BUMP_AMOUNT` (4_000_000 ledgers = 231.48 days at
    // 5s/ledger), the entry is on its final live ledger (`get_ttl == 0`).
    f.env.ledger().set_sequence_number(start_seq + BUMP_AMOUNT);
    f.env.as_contract(&f.manager, || {
        assert_eq!(f.env.storage().persistent().get_ttl(&senior_key), 0);
    });

    // Advance to the full 365-day maximum term: 31_536_000s = 6_307_200 ledgers
    // at 5s/ledger, which is 2_307_200 ledgers past `BUMP_AMOUNT`.
    let max_term_ledgers: u32 = (TERM / 5) as u32;
    assert!(max_term_ledgers > BUMP_AMOUNT);
    f.advance(TERM);
    f.env
        .ledger()
        .set_sequence_number(start_seq + max_term_ledgers);

    // Because `Position` is stored in `persistent()` storage (not `temporary()`),
    // expiry archives the entry rather than deleting it. In Protocol 23+ (SDK 27),
    // accessing an archived persistent entry restores it to
    // `min_persistent_entry_ttl - 1` ledgers and charges a persistent rent bump.
    let pos = f.client().position_of(&f.user, &Tranche::Senior);
    assert_eq!(pos.principal, 100_000_000);
    assert!(
        f.env
            .cost_estimate()
            .resources()
            .persistent_entry_rent_bumps
            >= 1
    );

    let min_persistent_ttl = f.env.ledger().get().min_persistent_entry_ttl - 1;
    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&senior_key),
            min_persistent_ttl
        );
    });

    // Settle and claim after archival: the depositor recovers their full settled
    // payout, and `claim` re-bumps the cleared position entry to BUMP_AMOUNT.
    f.client().settle();
    let balance_before = TokenClient::new(&f.env, &f.asset).balance(&f.user);
    let claimed = f.client().claim(&f.user, &Tranche::Senior);
    let balance_after = TokenClient::new(&f.env, &f.asset).balance(&f.user);

    assert_eq!(claimed, 100_000_000);
    assert_eq!(balance_after - balance_before, claimed);
    assert_eq!(
        f.client().position_of(&f.user, &Tranche::Senior).principal,
        0
    );
    f.env.as_contract(&f.manager, || {
        assert_eq!(
            f.env.storage().persistent().get_ttl(&senior_key),
            BUMP_AMOUNT
        );
    });
}
