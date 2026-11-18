//! A one-year Strata epoch through the Blend adapter, on captured testnet state.

use kanatoko::soroban_env_host::{
    storage::AccessType,
    xdr::{Hash, LedgerKey, LedgerKeyContractCode},
};
use kanatoko::soroban_sdk::testutils::storage::Persistent as _;
use kanatoko::soroban_sdk::testutils::Ledger as _;
use kanatoko::soroban_sdk::token::TokenClient;
use kanatoko::soroban_sdk::{vec, Address, BytesN, Env, InvokeError, Symbol};
use kanatoko::{testnet, ScenarioFork};
use std::rc::Rc;

mod adapter {
    use kanatoko::soroban_sdk;
    soroban_sdk::contractimport!(file = "wasm/strata_blend_adapter.wasm",);
}

mod manager {
    use kanatoko::soroban_sdk;
    soroban_sdk::contractimport!(file = "wasm/strata_epoch_manager.wasm",);
}

const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/testnet-blend.json");
const POOL: &str = "CCEBVDYM32YNYCVNRXQKDFFPISJJCV557CDZEIRBEE4NCV4KHPQ44HGF";
const XLM: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
const DAY: u64 = 86_400;
const YEAR: u64 = 365 * DAY;
const XLM_UNIT: i128 = 10_000_000;
const RATE_SCALE: i128 = 1_000_000_000_000;
/// Blend's user-position TTL in ledgers (~120 days), renewed on read below ~100 days.
const POSITION_TTL: u32 = 2_073_600;
/// Blend's `InvalidUtilRate`: a withdrawal that would leave utilisation at 100%.
const INVALID_UTIL_RATE: u32 = 1207;
const SUPPLY: u32 = 0;
const WITHDRAW: u32 = 1;

#[test]
fn one_year_epoch_on_the_testnet_pool() {
    testnet().cache(BUNDLE).offline().run(scenario).unwrap();
}

fn scenario(fork: &ScenarioFork<'_>) {
    let env = fork.env();
    fork.mock_all_auths();
    let pool = fork.contract(POOL);
    let xlm = fork.contract(XLM);
    let admin = fork.local_account("admin");
    let senior = fork.local_account("senior");
    let junior = fork.local_account("junior");
    let lender = fork.local_account("lender");
    let token = TokenClient::new(env, &xlm);
    fork.fund_local_account(&senior, 2_000 * XLM_UNIT as i64);
    fork.fund_local_account(&junior, 2_000 * XLM_UNIT as i64);
    fork.fund_local_account(&lender, 1_000 * XLM_UNIT as i64);

    // Blend clamps an over-sized withdrawal: it pays what the position holds and succeeds.
    submit(fork, &lender, SUPPLY, 100 * XLM_UNIT);
    let before = token.balance(&lender);
    let left = submit(fork, &lender, WITHDRAW, 200 * XLM_UNIT);
    let paid = token.balance(&lender) - before;
    assert!(left.supply.is_empty() && paid > 99 * XLM_UNIT && paid <= 100 * XLM_UNIT);

    // The two contracts name each other, so both addresses are derived first, as upstream does.
    let manager_id = deployed_address(env, &admin, 1);
    let adapter_id = deployed_address(env, &admin, 2);
    // These two executables are explicit local inputs, not network code.
    // Compiler-dependent WASM hashes must not require a new network capture.
    // Only their exact code keys are overridden; unknown network reads still fail.
    for wasm in [adapter::WASM, manager::WASM] {
        let hash = env
            .crypto()
            .sha256(&kanatoko::soroban_sdk::Bytes::from_slice(env, wasm));
        env.host()
            .setup_storage_entry(
                Rc::new(LedgerKey::ContractCode(LedgerKeyContractCode {
                    hash: Hash(hash.to_array()),
                })),
                None,
                AccessType::ReadWrite,
            )
            .unwrap();
    }
    env.register_at(&adapter_id, adapter::WASM, (&pool, &xlm, &manager_id));
    env.register_at(&manager_id, manager::WASM, (&admin, &adapter_id));
    // Declare the already-installed executables as local code to Kanatoko's
    // final coverage check. Replacing with the same bytes preserves the
    // current constructors' state and does not rerun either constructor.
    fork.replace_wasm(&adapter_id, adapter::WASM);
    fork.replace_wasm(&manager_id, manager::WASM);
    let manager = manager::Client::new(env, &manager_id);
    let adapter = adapter::Client::new(env, &adapter_id);
    manager.create_epoch(&YEAR, &500, &10_000);
    manager.deposit(&junior, &manager::Tranche::Junior, &(1_000 * XLM_UNIT));
    manager.deposit(&senior, &manager::Tranche::Senior, &(1_000 * XLM_UNIT));

    // A keeper calling `refresh` every 90 days renews the pool's ~120-day record of the adapter.
    assert_eq!(position_ttl(fork, &adapter_id), POSITION_TTL);
    for _ in 0..4 {
        advance(env, 90 * DAY);
        assert_eq!(position_ttl(fork, &adapter_id), POSITION_TTL - 1_555_200);
        adapter.refresh();
        assert_eq!(position_ttl(fork, &adapter_id), POSITION_TTL);
    }
    advance(env, 5 * DAY);

    // At maturity accrued liabilities exceed supply, so the pool refuses withdrawals.
    let reserve: adapter::Reserve = fork.invoke(&pool, "get_reserve", (&xlm,));
    let supplied = reserve.data.b_supply * reserve.data.b_rate / RATE_SCALE;
    let owed = reserve.data.d_supply * reserve.data.d_rate / RATE_SCALE;
    assert!(owed > supplied);
    let blocked = Err(Err(InvokeError::Contract(INVALID_UTIL_RATE)));
    assert_eq!(manager.try_settle(), blocked);
    let epoch = manager.current_epoch().unwrap();
    assert_eq!(epoch.status, manager::Status::Open);

    // Once new supply covers the shortfall, settlement pays the pool's own valuation.
    let top_up = (owed - supplied) * 11 / 10;
    fork.fund_local_account(&lender, top_up as i64);
    submit(fork, &lender, SUPPLY, top_up);
    let position: adapter::Positions = fork.invoke(&pool, "get_positions", (&adapter_id,));
    let value = position.supply.get_unchecked(0) * reserve.data.b_rate / RATE_SCALE;
    manager.settle();
    let epoch = manager.current_epoch().unwrap();
    assert_eq!(epoch.status, manager::Status::Settled);
    assert_eq!(epoch.value_redeemed, value);
    assert_eq!(epoch.senior_payout, 1_050 * XLM_UNIT);
    assert_eq!(epoch.junior_payout, value - 1_050 * XLM_UNIT);
    let senior_paid = manager.claim(&senior, &manager::Tranche::Senior);
    assert_eq!(senior_paid, epoch.senior_payout);
    let junior_paid = manager.claim(&junior, &manager::Tranche::Junior);
    assert_eq!(junior_paid, epoch.junior_payout);
}

fn submit(fork: &ScenarioFork<'_>, from: &Address, kind: u32, amount: i128) -> adapter::Positions {
    let request = adapter::Request {
        request_type: kind,
        address: fork.contract(XLM),
        amount,
    };
    let requests = vec![fork.env(), request];
    fork.invoke(&fork.contract(POOL), "submit", (from, from, from, requests))
}

fn position_ttl(fork: &ScenarioFork<'_>, account: &Address) -> u32 {
    let env = fork.env();
    let key = (Symbol::new(env, "Positions"), account.clone());
    env.as_contract(&fork.contract(POOL), || {
        env.storage().persistent().get_ttl(&key)
    })
}

fn deployed_address(env: &Env, deployer: &Address, salt: u8) -> Address {
    let salt = BytesN::from_array(env, &[salt; 32]);
    let deployer = env.deployer().with_address(deployer.clone(), salt);
    deployer.deployed_address()
}

fn advance(env: &Env, seconds: u64) {
    env.ledger().with_mut(|ledger| {
        ledger.timestamp += seconds;
        ledger.sequence_number += (seconds / 5) as u32;
    });
}
