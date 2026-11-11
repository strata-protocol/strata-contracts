//! Resource budget measurements for every [`EpochManager`] entrypoint.
//!
//! Uses the pinned `soroban-sdk` (`27.0.6`) [`CostEstimate`] metering API
//! (`env.cost_estimate().resources()` and `env.cost_estimate().fee()`) to
//! record CPU instructions, memory bytes, ledger read/write entry counts,
//! read/write byte footprints, and contract event sizes across `N = 1`, `10`,
//! and `100` depositor positions.
//!
//! Run with `--nocapture` to print the tables reproduced in
//! [`docs/resource-report.md`](../../docs/resource-report.md):
//!
//! ```text
//! cargo test -p strata-tests resource_budget -- --nocapture
//! ```

use soroban_sdk::{
    testutils::{Address as _, EnvTestConfig, Ledger},
    token::StellarAssetClient,
    Address, Env,
};
use strata_blend_adapter::BlendAdapter;
use strata_blend_pool_mock::{MockPool, MockPoolClient};
use strata_epoch_manager::{EpochManager, EpochManagerClient, Status, Tranche};
use strata_mock_vault::{MockVault, MockVaultClient};
use strata_waterfall::SECONDS_PER_YEAR;

const TERM: u64 = SECONDS_PER_YEAR;
const RATE: u32 = 500; // 5%/yr senior target
const RATIO: u32 = 10_000; // 1.0x senior cap
const DEPOSIT_AMOUNT: i128 = 1_000_000;
const STRATEGY_RESERVE: i128 = 1_000_000_000_000_000_000;
const ONE_TO_ONE: i128 = 1_000_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrypointSample {
    pub entrypoint: &'static str,
    pub positions: usize,
    pub instructions: i64,
    pub mem_bytes: i64,
    pub memory_read_entries: u32,
    pub disk_read_entries: u32,
    pub write_entries: u32,
    pub disk_read_bytes: u32,
    pub write_bytes: u32,
    pub contract_events_size_bytes: u32,
    pub persistent_entry_rent_bumps: u32,
    pub fee_total_stroops: i64,
}

impl EntrypointSample {
    pub fn total_footprint_entries(&self) -> u32 {
        self.memory_read_entries + self.disk_read_entries + self.write_entries
    }
}

fn clean_env() -> Env {
    Env::new_with_config(EnvTestConfig {
        capture_snapshot_at_drop: false,
    })
}

fn capture(env: &Env, entrypoint: &'static str, positions: usize) -> EntrypointSample {
    let res = env.cost_estimate().resources();
    let fee = env.cost_estimate().fee();
    EntrypointSample {
        entrypoint,
        positions,
        instructions: res.instructions,
        mem_bytes: res.mem_bytes,
        memory_read_entries: res.memory_read_entries,
        disk_read_entries: res.disk_read_entries,
        write_entries: res.write_entries,
        disk_read_bytes: res.disk_read_bytes,
        write_bytes: res.write_bytes,
        contract_events_size_bytes: res.contract_events_size_bytes,
        persistent_entry_rent_bumps: res.persistent_entry_rent_bumps,
        fee_total_stroops: fee.total,
    }
}

/// Measures every state-changing entrypoint and read-only view on `EpochManager`
/// backed by `MockVault` with `n` positions (`n` total depositor positions:
/// for `n == 1`, 1 Junior position; for `n > 1`, `n / 2` Junior positions and
/// `n / 2` Senior positions).
fn measure_mock_vault_lifecycle(n: usize) -> Vec<EntrypointSample> {
    assert!(n == 1 || n.is_multiple_of(2), "n must be 1 or even");
    let env = clean_env();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let vault = env.register(MockVault, (admin.clone(), asset.clone()));
    StellarAssetClient::new(&env, &asset).mint(&vault, &STRATEGY_RESERVE);

    let mut samples = Vec::new();

    // 1. __constructor (measured via env.register)
    let manager = env.register(EpochManager, (admin.clone(), vault.clone()));
    samples.push(capture(&env, "__constructor", n));

    let mgr = EpochManagerClient::new(&env, &manager);
    let vault_client = MockVaultClient::new(&env, &vault);
    let asset_admin = StellarAssetClient::new(&env, &asset);

    // 2. create_epoch (before depositors arrive)
    mgr.create_epoch(&TERM, &RATE, &RATIO);
    samples.push(capture(&env, "create_epoch", n));

    // Pre-generate and fund depositors for the deposit window.
    let (junior_count, senior_count) = if n == 1 { (1, 0) } else { (n / 2, n / 2) };
    let mut juniors = Vec::with_capacity(junior_count);
    let mut seniors = Vec::with_capacity(senior_count);

    for _ in 0..junior_count {
        let who = Address::generate(&env);
        asset_admin.mint(&who, &(DEPOSIT_AMOUNT * 10));
        juniors.push(who);
    }
    for _ in 0..senior_count {
        let who = Address::generate(&env);
        asset_admin.mint(&who, &(DEPOSIT_AMOUNT * 10));
        seniors.push(who);
    }

    // 3. deposit (Junior) & deposit (Senior)
    let mut last_junior_sample = None;
    for who in &juniors {
        mgr.deposit(who, &Tranche::Junior, &DEPOSIT_AMOUNT);
        last_junior_sample = Some(capture(&env, "deposit (Junior)", n));
    }
    samples.push(last_junior_sample.expect("at least one junior deposit"));

    if senior_count > 0 {
        let mut last_senior_sample = None;
        for who in &seniors {
            mgr.deposit(who, &Tranche::Senior, &DEPOSIT_AMOUNT);
            last_senior_sample = Some(capture(&env, "deposit (Senior)", n));
        }
        samples.push(last_senior_sample.expect("at least one senior deposit"));
    } else {
        // For n == 1, measure `deposit (Senior)` on an isolated 1-Junior epoch so
        // the main `n == 1` environment retains strictly 1 position for the
        // remaining views, settlement, and claim measurements.
        let sep_env = clean_env();
        sep_env.mock_all_auths();
        let sep_admin = Address::generate(&sep_env);
        let sep_asset = sep_env
            .register_stellar_asset_contract_v2(sep_admin.clone())
            .address();
        let sep_vault = sep_env.register(MockVault, (sep_admin.clone(), sep_asset.clone()));
        let sep_mgr_addr = sep_env.register(EpochManager, (sep_admin.clone(), sep_vault.clone()));
        let sep_mgr = EpochManagerClient::new(&sep_env, &sep_mgr_addr);
        let j_user = Address::generate(&sep_env);
        let s_user = Address::generate(&sep_env);
        StellarAssetClient::new(&sep_env, &sep_asset).mint(&j_user, &(DEPOSIT_AMOUNT * 2));
        StellarAssetClient::new(&sep_env, &sep_asset).mint(&s_user, &(DEPOSIT_AMOUNT * 2));
        sep_mgr.create_epoch(&TERM, &RATE, &RATIO);
        sep_mgr.deposit(&j_user, &Tranche::Junior, &DEPOSIT_AMOUNT);
        sep_mgr.deposit(&s_user, &Tranche::Senior, &DEPOSIT_AMOUNT);
        samples.push(capture(&sep_env, "deposit (Senior)", n));
    }

    // 4-12. Read-only views while Open (with `n` positions active in the epoch)
    let _ = mgr.admin();
    samples.push(capture(&env, "admin", n));

    let _ = mgr.vault();
    samples.push(capture(&env, "vault", n));

    let _ = mgr.asset();
    samples.push(capture(&env, "asset", n));

    let _ = mgr.current_epoch();
    samples.push(capture(&env, "current_epoch", n));

    let _ = mgr.position_of(&juniors[0], &Tranche::Junior);
    samples.push(capture(&env, "position_of (Open)", n));

    let _ = mgr.senior_room();
    samples.push(capture(&env, "senior_room", n));

    let _ = mgr.seconds_to_maturity();
    samples.push(capture(&env, "seconds_to_maturity", n));

    let _ = mgr.project(&(DEPOSIT_AMOUNT * (n as i128) * 11 / 10));
    samples.push(capture(&env, "project", n));

    let _ = mgr.vault_shares();
    samples.push(capture(&env, "vault_shares", n));

    // 13. Advance clock to maturity with 5% vault yield and measure `settle`
    vault_client.set_yield_rate(&500);
    let now = env.ledger().timestamp() + TERM;
    env.ledger().set_timestamp(now);

    mgr.settle();
    samples.push(capture(&env, "settle", n));

    // 14. Read-only `position_of` after settlement (uses settled payouts rather than calling vault)
    let _ = mgr.position_of(&juniors[0], &Tranche::Junior);
    samples.push(capture(&env, "position_of (Settled)", n));

    // 15-16. Claim all positions, recording the first claim (`mul_div` path when n > 1)
    // and the last claim in the tranche (remainder-absorbing path).
    let mut first_claim_sample = None;
    let mut last_claim_sample = None;
    for (idx, who) in juniors.iter().enumerate() {
        mgr.claim(who, &Tranche::Junior);
        if idx == 0 {
            first_claim_sample = Some(capture(&env, "claim (first in tranche)", n));
        }
        if idx + 1 == juniors.len() {
            last_claim_sample = Some(capture(&env, "claim (last in tranche)", n));
        }
    }
    for who in &seniors {
        mgr.claim(who, &Tranche::Senior);
    }
    samples.push(first_claim_sample.expect("first claim"));
    samples.push(last_claim_sample.expect("last claim"));

    // 17. close_epoch once all n positions have claimed
    mgr.close_epoch();
    samples.push(capture(&env, "close_epoch", n));

    assert_eq!(mgr.current_epoch().unwrap().status, Status::Closed);
    samples
}

/// Measures the vault-calling entrypoints (`__constructor`, `deposit (Junior)`,
/// `deposit (Senior)`, `position_of (Open)`, `settle`) when `EpochManager` wraps
/// `BlendAdapter` + `MockPool` across `n = 1, 10, 100` positions.
fn measure_blend_adapter_lifecycle(n: usize) -> Vec<EntrypointSample> {
    assert!(n == 1 || n.is_multiple_of(2), "n must be 1 or even");
    let env = clean_env();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let asset = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let manager_id = Address::generate(&env);
    let adapter_id = Address::generate(&env);

    let pool = env.register(MockPool, (&asset, &ONE_TO_ONE));
    let adapter = env.register_at(&adapter_id, BlendAdapter, (&pool, &asset, &manager_id));

    let mut samples = Vec::new();
    let manager = env.register_at(&manager_id, EpochManager, (&admin, &adapter));
    samples.push(capture(&env, "__constructor (BlendAdapter)", n));

    let mgr = EpochManagerClient::new(&env, &manager);
    let pool_client = MockPoolClient::new(&env, &pool);
    let asset_admin = StellarAssetClient::new(&env, &asset);

    mgr.create_epoch(&TERM, &RATE, &RATIO);

    let (junior_count, senior_count) = if n == 1 { (1, 0) } else { (n / 2, n / 2) };
    let mut juniors = Vec::with_capacity(junior_count);
    let mut seniors = Vec::with_capacity(senior_count);

    for _ in 0..junior_count {
        let who = Address::generate(&env);
        asset_admin.mint(&who, &(DEPOSIT_AMOUNT * 10));
        juniors.push(who);
    }
    for _ in 0..senior_count {
        let who = Address::generate(&env);
        asset_admin.mint(&who, &(DEPOSIT_AMOUNT * 10));
        seniors.push(who);
    }

    let mut last_junior = None;
    for who in &juniors {
        mgr.deposit(who, &Tranche::Junior, &DEPOSIT_AMOUNT);
        last_junior = Some(capture(&env, "deposit (Junior, BlendAdapter)", n));
    }
    samples.push(last_junior.unwrap());

    if senior_count > 0 {
        let mut last_senior = None;
        for who in &seniors {
            mgr.deposit(who, &Tranche::Senior, &DEPOSIT_AMOUNT);
            last_senior = Some(capture(&env, "deposit (Senior, BlendAdapter)", n));
        }
        samples.push(last_senior.unwrap());
    }

    let _ = mgr.position_of(&juniors[0], &Tranche::Junior);
    samples.push(capture(&env, "position_of (Open, BlendAdapter)", n));

    // 10% gain in the pool (b_rate = 1.1e12), pre-fund pool reserve for payout
    asset_admin.mint(&pool, &STRATEGY_RESERVE);
    pool_client.set_b_rate(&1_100_000_000_000);
    let now = env.ledger().timestamp() + TERM;
    env.ledger().set_timestamp(now);

    mgr.settle();
    samples.push(capture(&env, "settle (BlendAdapter)", n));

    samples
}

struct MainnetLimits {
    instructions: i64,
    mem_bytes: i64,
    disk_read_entries: u32,
    write_entries: u32,
    ledger_entries: u32,
    disk_read_bytes: u32,
    write_bytes: u32,
    contract_events_size_bytes: u32,
}

/// Stellar Mainnet per-transaction invocation resource limits enforced by
/// `Env::default()` in `soroban-sdk 27.0.6` (`NetworkInvocationResourceLimits::mainnet()`).
const MAINNET_LIMITS: MainnetLimits = MainnetLimits {
    instructions: 400_000_000,
    mem_bytes: 41_943_040,
    disk_read_entries: 200,
    write_entries: 200,
    ledger_entries: 400,
    disk_read_bytes: 200_000,
    write_bytes: 132_096,
    contract_events_size_bytes: 16_384,
};

#[test]
fn every_epoch_manager_entrypoint_stays_well_within_mainnet_resource_limits_and_is_o1_in_depositors(
) {
    let limits = MAINNET_LIMITS;

    let run_1 = measure_mock_vault_lifecycle(1);
    let run_10 = measure_mock_vault_lifecycle(10);
    let run_100 = measure_mock_vault_lifecycle(100);

    println!("\n=== Mainnet Per-Transaction Resource Limits (soroban-sdk 27.0.6) ===");
    println!(
        "instructions={} | mem_bytes={} | disk_read_entries={} | write_entries={} | ledger_entries={} | disk_read_bytes={} | write_bytes={} | contract_events_size_bytes={}",
        limits.instructions,
        limits.mem_bytes,
        limits.disk_read_entries,
        limits.write_entries,
        limits.ledger_entries,
        limits.disk_read_bytes,
        limits.write_bytes,
        limits.contract_events_size_bytes,
    );

    println!("\n=== EpochManager + MockVault Resource Table (N = 1, 10, 100) ===");
    println!(
        "| Entrypoint | N | CPU instructions | % of 400M | Memory (bytes) | % of 40 MiB | Mem Read Entries | Disk Read Entries | Write Entries | Total Footprint (<=400) | Write Bytes (<=132,096) | Event Bytes (<=16,384) | Rent Bumps | Est. Fee (stroops) |"
    );
    println!(
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    );

    for idx in 0..run_1.len() {
        for sample in [&run_1[idx], &run_10[idx], &run_100[idx]] {
            let cpu_pct = (sample.instructions as f64 / limits.instructions as f64) * 100.0;
            let mem_pct = (sample.mem_bytes as f64 / limits.mem_bytes as f64) * 100.0;
            println!(
                "| `{}` | {} | {} | {:.4}% | {} | {:.4}% | {} | {} | {} | {} | {} | {} | {} | {} |",
                sample.entrypoint,
                sample.positions,
                sample.instructions,
                cpu_pct,
                sample.mem_bytes,
                mem_pct,
                sample.memory_read_entries,
                sample.disk_read_entries,
                sample.write_entries,
                sample.total_footprint_entries(),
                sample.write_bytes,
                sample.contract_events_size_bytes,
                sample.persistent_entry_rent_bumps,
                sample.fee_total_stroops,
            );

            // Assert every entrypoint stays well within Mainnet limits (in fact < 3% of every limit).
            assert!(
                sample.instructions < limits.instructions / 50,
                "{} exceeded 2% of CPU instruction limit",
                sample.entrypoint
            );
            assert!(
                sample.mem_bytes < limits.mem_bytes / 20,
                "{} exceeded 5% of memory byte limit",
                sample.entrypoint
            );
            assert!(
                sample.disk_read_entries <= limits.disk_read_entries,
                "{} exceeded disk_read_entries",
                sample.entrypoint
            );
            assert!(
                sample.write_entries <= limits.write_entries,
                "{} exceeded write_entries",
                sample.entrypoint
            );
            assert!(
                sample.total_footprint_entries() <= limits.ledger_entries,
                "{} exceeded total ledger_entries",
                sample.entrypoint
            );
            assert!(
                sample.disk_read_bytes <= limits.disk_read_bytes,
                "{} exceeded disk_read_bytes",
                sample.entrypoint
            );
            assert!(
                sample.write_bytes <= limits.write_bytes,
                "{} exceeded write_bytes",
                sample.entrypoint
            );
            assert!(
                sample.contract_events_size_bytes <= limits.contract_events_size_bytes,
                "{} exceeded contract_events_size_bytes",
                sample.entrypoint
            );
        }

        // Assert O(1) ledger footprint & byte sizes between N = 10 and N = 100.
        let s10 = &run_10[idx];
        let s100 = &run_100[idx];
        assert_eq!(
            s10.memory_read_entries, s100.memory_read_entries,
            "{} memory_read_entries grew from N=10 to N=100",
            s10.entrypoint
        );
        assert_eq!(
            s10.disk_read_entries, s100.disk_read_entries,
            "{} disk_read_entries grew from N=10 to N=100",
            s10.entrypoint
        );
        assert_eq!(
            s10.write_entries, s100.write_entries,
            "{} write_entries grew from N=10 to N=100",
            s10.entrypoint
        );
        assert_eq!(
            s10.write_bytes, s100.write_bytes,
            "{} write_bytes grew from N=10 to N=100",
            s10.entrypoint
        );
        assert_eq!(
            s10.contract_events_size_bytes, s100.contract_events_size_bytes,
            "{} contract_events_size_bytes grew from N=10 to N=100",
            s10.entrypoint
        );
    }

    let blend_1 = measure_blend_adapter_lifecycle(1);
    let blend_10 = measure_blend_adapter_lifecycle(10);
    let blend_100 = measure_blend_adapter_lifecycle(100);

    println!("\n=== EpochManager + BlendAdapter Resource Table (N = 1, 10, 100) ===");
    for s in blend_1
        .iter()
        .chain(blend_10.iter())
        .chain(blend_100.iter())
    {
        let cpu_pct = (s.instructions as f64 / limits.instructions as f64) * 100.0;
        let mem_pct = (s.mem_bytes as f64 / limits.mem_bytes as f64) * 100.0;
        println!(
            "| `{}` | {} | {} | {:.4}% | {} | {:.4}% | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.entrypoint,
            s.positions,
            s.instructions,
            cpu_pct,
            s.mem_bytes,
            mem_pct,
            s.memory_read_entries,
            s.disk_read_entries,
            s.write_entries,
            s.total_footprint_entries(),
            s.write_bytes,
            s.contract_events_size_bytes,
            s.persistent_entry_rent_bumps,
            s.fee_total_stroops,
        );
    }
    // Verify settle (BlendAdapter) footprint is strictly O(1) from N=10 to N=100
    assert_eq!(
        blend_10.last().unwrap().total_footprint_entries(),
        blend_100.last().unwrap().total_footprint_entries()
    );
    assert_eq!(
        blend_10.last().unwrap().write_bytes,
        blend_100.last().unwrap().write_bytes
    );
}

#[test]
fn host_storage_map_cloning_explains_in_memory_instruction_slope_not_contract_loops() {
    // In `soroban-env-host`'s test environment, `Storage.map` retains every
    // ledger entry ever written in the `Env` inside a `MeteredOrdMap` (rather
    // than loading only the transaction's declared footprint as a live validator
    // does). Each depositor in `measure_mock_vault_lifecycle` adds 3 persistent/
    // temporary entries to `Env`'s `Storage.map` (token `Balance`, auth `Nonce`,
    // and `DataKey::Position`), which costs exactly 340 bytes of `MeteredOrdMap`
    // clone allocation per sub-call frame in `soroban-env-host 27.0.1` — even on
    // views like `admin()` that read only `DataKey::Config` and never touch
    // `Epoch` or `Position`.
    let run_1 = measure_mock_vault_lifecycle(1);
    let run_10 = measure_mock_vault_lifecycle(10);
    let run_100 = measure_mock_vault_lifecycle(100);

    const BYTES_PER_DEPOSITOR_PER_FRAME: i64 = 340;

    for name in [
        "admin",
        "vault",
        "asset",
        "current_epoch",
        "senior_room",
        "seconds_to_maturity",
        "project",
        "vault_shares",
        "position_of (Settled)",
    ] {
        let s1 = run_1.iter().find(|s| s.entrypoint == name).unwrap();
        let s10 = run_10.iter().find(|s| s.entrypoint == name).unwrap();
        let s100 = run_100.iter().find(|s| s.entrypoint == name).unwrap();

        assert_eq!(
            s10.mem_bytes - s1.mem_bytes,
            9 * BYTES_PER_DEPOSITOR_PER_FRAME,
            "{name} N=1->10 delta must equal 9 * 340 B of test-host Storage.map clone"
        );
        assert_eq!(
            s100.mem_bytes - s1.mem_bytes,
            99 * BYTES_PER_DEPOSITOR_PER_FRAME,
            "{name} N=1->100 delta must equal 99 * 340 B of test-host Storage.map clone"
        );
    }

    // `position_of (Open)` on MockVault executes 2 contract frames
    // (`EpochManager::position_of` -> `MockVault::convert_to_assets`), so the
    // test-host map clone is `2 * 340 = 680` bytes per depositor.
    let pos1 = run_1
        .iter()
        .find(|s| s.entrypoint == "position_of (Open)")
        .unwrap();
    let pos10 = run_10
        .iter()
        .find(|s| s.entrypoint == "position_of (Open)")
        .unwrap();
    let pos100 = run_100
        .iter()
        .find(|s| s.entrypoint == "position_of (Open)")
        .unwrap();
    assert_eq!(
        pos10.mem_bytes - pos1.mem_bytes,
        9 * 2 * BYTES_PER_DEPOSITOR_PER_FRAME
    );
    assert_eq!(
        pos100.mem_bytes - pos1.mem_bytes,
        99 * 2 * BYTES_PER_DEPOSITOR_PER_FRAME
    );

    // `position_of (Open, BlendAdapter)` executes 4 contract frames
    // (`EpochManager` -> `BlendAdapter` -> `MockPool::get_positions` +
    // `MockPool::get_reserve`), so the test-host map clone is `4 * 340 = 1,360`
    // bytes per depositor.
    let blend_1 = measure_blend_adapter_lifecycle(1);
    let blend_10 = measure_blend_adapter_lifecycle(10);
    let blend_100 = measure_blend_adapter_lifecycle(100);
    let bpos1 = blend_1
        .iter()
        .find(|s| s.entrypoint == "position_of (Open, BlendAdapter)")
        .unwrap();
    let bpos10 = blend_10
        .iter()
        .find(|s| s.entrypoint == "position_of (Open, BlendAdapter)")
        .unwrap();
    let bpos100 = blend_100
        .iter()
        .find(|s| s.entrypoint == "position_of (Open, BlendAdapter)")
        .unwrap();
    assert_eq!(
        bpos10.mem_bytes - bpos1.mem_bytes,
        9 * 4 * BYTES_PER_DEPOSITOR_PER_FRAME
    );
    assert_eq!(
        bpos100.mem_bytes - bpos1.mem_bytes,
        99 * 4 * BYTES_PER_DEPOSITOR_PER_FRAME
    );
}
