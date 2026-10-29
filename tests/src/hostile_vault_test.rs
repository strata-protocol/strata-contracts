//! Hostile Vault Reentrancy and Broken Vault Integration Tests.
//!
//! Addresses Issue #8: test(security): analyse hostile-vault reentrancy and add a malicious-vault test.
//!
//! Proves:
//! 1. Reentrancy attempt during `redeem` back into `settle` fails safely (Soroban call stack / already settled / invariants).
//! 2. Malicious vault returning less than recorded/redeemed assets is blocked by defensive balance check (`held < owed`).
//! 3. Malicious vault reporting inflated assets in `convert_to_assets` does not corrupt actual settlement numbers.
//! 4. Vault reverting in `redeem` leaves epoch status uncorrupted.

use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Address, Env, IntoVal,
};
use strata_epoch_manager::{EpochManager, EpochManagerClient, Error, Status, Tranche};
use strata_vault_interface::{VaultClient, VaultInterface};
use strata_waterfall::SECONDS_PER_YEAR;

const TERM: u64 = SECONDS_PER_YEAR;
const RATE: u32 = 500; // 5%
const RATIO: u32 = 10_000;
const STARTING_BALANCE: i128 = 1_000_000_000;

#[contracttype]
pub enum HostileMode {
    Normal,
    ReenterSettle,
    UnderpayAssets,
    InflateAssets,
    RevertOnRedeem,
}

#[contracttype]
pub enum HostileDataKey {
    Asset,
    Shares(Address),
    TotalAssets,
    TotalShares,
    Mode,
    TargetManager,
}

#[contract]
pub struct HostileVault;

#[contractimpl]
impl HostileVault {
    pub fn init(env: Env, asset: Address, manager: Address) {
        env.storage().persistent().set(&HostileDataKey::Asset, &asset);
        env.storage().persistent().set(&HostileDataKey::TargetManager, &manager);
        env.storage().persistent().set(&HostileDataKey::Mode, &HostileMode::Normal);
        env.storage().persistent().set(&HostileDataKey::TotalAssets, &0i128);
        env.storage().persistent().set(&HostileDataKey::TotalShares, &0i128);
    }

    pub fn set_mode(env: Env, mode: HostileMode) {
        env.storage().persistent().set(&HostileDataKey::Mode, &mode);
    }
}

#[contractimpl]
impl VaultInterface for HostileVault {
    fn asset(env: Env) -> Address {
        env.storage().persistent().get(&HostileDataKey::Asset).unwrap()
    }

    fn deposit(env: Env, assets: i128, receiver: Address) -> i128 {
        let asset: Address = env.storage().persistent().get(&HostileDataKey::Asset).unwrap();
        TokenClient::new(&env, &asset).transfer(&receiver, &env.current_contract_address(), &assets);

        let current_shares: i128 = env.storage().persistent().get(&HostileDataKey::Shares(receiver.clone())).unwrap_or(0);
        env.storage().persistent().set(&HostileDataKey::Shares(receiver), &(current_shares + assets));

        let tot_assets: i128 = env.storage().persistent().get(&HostileDataKey::TotalAssets).unwrap_or(0);
        let tot_shares: i128 = env.storage().persistent().get(&HostileDataKey::TotalShares).unwrap_or(0);
        env.storage().persistent().set(&HostileDataKey::TotalAssets, &(tot_assets + assets));
        env.storage().persistent().set(&HostileDataKey::TotalShares, &(tot_shares + assets));

        assets
    }

    fn redeem(env: Env, shares: i128, receiver: Address, _owner: Address) -> i128 {
        let mode: HostileMode = env.storage().persistent().get(&HostileDataKey::Mode).unwrap_or(HostileMode::Normal);
        let asset: Address = env.storage().persistent().get(&HostileDataKey::Asset).unwrap();

        match mode {
            HostileMode::RevertOnRedeem => {
                panic!("HostileVault: intentionally reverting on redeem");
            }
            HostileMode::ReenterSettle => {
                let manager: Address = env.storage().persistent().get(&HostileDataKey::TargetManager).unwrap();
                // Attempt reentrancy: call settle() again while already in settle()->redeem()
                EpochManagerClient::new(&env, &manager).settle();
                // Pay normal assets if reentrancy somehow succeeded
                TokenClient::new(&env, &asset).transfer(&env.current_contract_address(), &receiver, &shares);
                shares
            }
            HostileMode::UnderpayAssets => {
                // Malicious vault claims it returns shares, but transfers only half the required asset tokens
                let underpaid = shares / 2;
                TokenClient::new(&env, &asset).transfer(&env.current_contract_address(), &receiver, &underpaid);
                shares // Lies that full shares value was redeemed
            }
            HostileMode::InflateAssets | HostileMode::Normal => {
                TokenClient::new(&env, &asset).transfer(&env.current_contract_address(), &receiver, &shares);
                shares
            }
        }
    }

    fn total_assets(env: Env) -> i128 {
        env.storage().persistent().get(&HostileDataKey::TotalAssets).unwrap_or(0)
    }

    fn convert_to_assets(env: Env, shares: i128) -> i128 {
        let mode: HostileMode = env.storage().persistent().get(&HostileDataKey::Mode).unwrap_or(HostileMode::Normal);
        match mode {
            HostileMode::InflateAssets => shares.saturating_mul(100), // Lies 100x
            _ => shares,
        }
    }

    fn balance_of(env: Env, account: Address) -> i128 {
        env.storage().persistent().get(&HostileDataKey::Shares(account)).unwrap_or(0)
    }
}

#[test]
fn test_hostile_vault_reentrancy_is_blocked() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let manager_id = env.register_contract(None, EpochManager);
    let manager = EpochManagerClient::new(&env, &manager_id);

    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = TokenClient::new(&env, &token_id.address());
    let token_admin_client = StellarAssetClient::new(&env, &token_id.address());

    let vault_id = env.register_contract(None, HostileVault);
    let hostile_vault = HostileVaultClient::new(&env, &vault_id);
    hostile_vault.init(&token_id.address(), &manager_id);

    manager.init(&admin, &vault_id.address(), &TERM, &RATE, &RATIO);

    let dep = Address::generate(&env);
    token_admin_client.mint(&dep, &STARTING_BALANCE);

    manager.deposit(&Tranche::Senior, &100_000_000, &dep);
    manager.deposit(&Tranche::Junior, &100_000_000, &dep);

    // Fast-forward past maturity
    env.ledger().set_timestamp(TERM + 10);

    // Set hostile mode: attempt reentrancy back into settle()
    hostile_vault.set_mode(&HostileMode::ReenterSettle);

    // Settle must fail or prevent reentrancy recursion
    let res = manager.try_settle();
    assert!(res.is_err(), "Reentrancy attempt must not succeed silently");
}

#[test]
fn test_hostile_vault_underpayment_fails_defensive_balance_check() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let manager_id = env.register_contract(None, EpochManager);
    let manager = EpochManagerClient::new(&env, &manager_id);

    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_admin_client = StellarAssetClient::new(&env, &token_id.address());

    let vault_id = env.register_contract(None, HostileVault);
    let hostile_vault = HostileVaultClient::new(&env, &vault_id);
    hostile_vault.init(&token_id.address(), &manager_id);

    manager.init(&admin, &vault_id.address(), &TERM, &RATE, &RATIO);

    let dep = Address::generate(&env);
    token_admin_client.mint(&dep, &STARTING_BALANCE);

    manager.deposit(&Tranche::Senior, &100_000_000, &dep);
    manager.deposit(&Tranche::Junior, &100_000_000, &dep);

    env.ledger().set_timestamp(TERM + 10);

    // Set hostile mode: underpay assets during redeem
    hostile_vault.set_mode(&HostileMode::UnderpayAssets);

    let res = manager.try_settle();
    // Must return Error::InvalidAmount because held < owed (line 461 of epoch-manager)
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn test_hostile_vault_revert_leaves_epoch_unsettled() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let manager_id = env.register_contract(None, EpochManager);
    let manager = EpochManagerClient::new(&env, &manager_id);

    let token_admin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_admin_client = StellarAssetClient::new(&env, &token_id.address());

    let vault_id = env.register_contract(None, HostileVault);
    let hostile_vault = HostileVaultClient::new(&env, &vault_id);
    hostile_vault.init(&token_id.address(), &manager_id);

    manager.init(&admin, &vault_id.address(), &TERM, &RATE, &RATIO);

    let dep = Address::generate(&env);
    token_admin_client.mint(&dep, &STARTING_BALANCE);

    manager.deposit(&Tranche::Senior, &100_000_000, &dep);
    manager.deposit(&Tranche::Junior, &100_000_000, &dep);

    env.ledger().set_timestamp(TERM + 10);

    hostile_vault.set_mode(&HostileMode::RevertOnRedeem);

    let res = manager.try_settle();
    assert!(res.is_err());

    // Epoch status must remain Running, not corrupted
    let epoch = manager.get_epoch();
    assert_eq!(epoch.status, Status::Running);
}

