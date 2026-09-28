//! A test-only ERC-4626-style vault whose yield rate the admin sets, including
//! negative.
//!
//! **This is a mock. It is not a production vault and must never hold real
//! value.** It exists so the integration tests can drive a good epoch, a flat
//! epoch and a bad epoch deterministically, by moving the ledger clock and
//! setting a rate.
//!
//! # How yield works
//!
//! The vault keeps `total_assets` and accrues lazily on every interaction:
//!
//! ```text
//! delta      = total_assets * rate_bps * elapsed / (10_000 * SECONDS_PER_YEAR)
//! total_assets = max(0, total_assets + delta)
//! ```
//!
//! Three properties matter:
//!
//! - **Negative rates are first-class.** `rate_bps` is an `i32`, so the admin
//!   can set -5_000 for a 5%/yr loss. There is no special negative path.
//! - **The yield base is `total_assets` itself**, so the vault compounds
//!   across accruals, and a vault that has been wiped to zero can never
//!   conjure assets back: `delta` is proportional to a base of zero.
//! - **`total_assets` is clamped at zero** and at [`MAX_ASSETS`]. It is a
//!   vault; it cannot go negative, and the clamp is what keeps the accrual
//!   product inside `i128`.
//!
//! Accrual is lazy and coarse: `last_accrual_ts` is a single global timestamp,
//! not per-depositor, so a position earns yield for the whole period in which
//! it was held even if it arrived partway through. That is a simplification,
//! and it is the right one for a test double. Deposits and redemptions force
//! an accrual *before* changing the balance, so a position never earns for
//! time it was not held.
//!
//! # The mock's yield is unbacked, by design
//!
//! `total_assets` is an accounting figure. A real vault grows it by holding
//! something; the mock has nothing to hold, so a positive rate makes it claim
//! assets it does not have and a redemption of them fails for lack of balance.
//!
//! The tests handle this with a pre-funded **strategy reserve**: the test mints
//! spare tokens to the vault's address so it can always pay out. The reserve is
//! not counted in `total_assets`, which stays the authoritative figure every
//! test asserts on. What this means concretely is that the mock's real token
//! balance is not a meaningful assertion target — assert on `total_assets` and
//! on redemption return values instead.
//!
//! A negative rate needs no reserve: the vault retains the shortfall, which is
//! precisely what a loss looks like in cash terms.
//!
//! # Bounds
//!
//! These are tighter than the epoch manager's, deliberately: a mock that
//! accepts the same 1e26 ceiling as the manager could overflow its accrual
//! product. The widest product here is
//! `1e24 * 1e5 * 3.15e8 = 3.15e37 < i128::MAX = 1.7e38`. Overflow coverage at
//! the protocol's real ceiling comes from the waterfall property tests, not
//! from driving this contract to its limits.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env,
};
use strata_vault_interface::VaultInterface;

/// Basis-point denominator: 10_000 == 100% == 1.0x.
const BPS_DENOMINATOR: i128 = 10_000;
/// Seconds in a 365-day year, matching the waterfall spec.
/// Seconds in a 365-day year, matching the waterfall spec.
pub const SECONDS_PER_YEAR: u64 = 31_536_000;
const YEAR_DENOMINATOR: i128 = BPS_DENOMINATOR * SECONDS_PER_YEAR as i128;

/// Largest `total_assets` the mock will hold: 1e24 base units.
///
/// Far above any realistic test amount and far below the 1e26 the epoch
/// manager permits, specifically so the accrual product cannot overflow. The
/// widest product is
/// `1e24 * 1e5 * 3.15e8 = 3.15e37` against `i128::MAX = 1.7014e38`, leaving
/// roughly 5.4x headroom. `asset_ceiling_keeps_accrual_inside_i128` in the
/// tests pins that arithmetic, because a stray digit group here silently
/// invalidates the whole bound.
pub const MAX_ASSETS: i128 = 1_000_000_000_000_000_000_000_000;

/// Largest absolute yield rate, in basis points per year. 100_000 == 1000%/yr.
pub const MAX_ABS_RATE_BPS: i32 = 100_000;

/// Yield stops accruing after this much elapsed time in a single accrual
/// window. Past ten years the vault simply stops, which keeps `elapsed` inside
/// the bound the overflow argument depends on.
pub const MAX_ACCRUAL_SECONDS: u64 = 10 * SECONDS_PER_YEAR;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// An amount of assets or shares was zero where zero is meaningless.
    ZeroAmount = 1,
    /// A deposit would have to mint zero shares, so the assets would be lost.
    ZeroShares = 2,
    /// The account does not hold that many shares.
    InsufficientShares = 3,
    /// Arithmetic exceeded the documented bounds.
    ArithmeticOverflow = 4,
    /// `total_assets` is already at the mock's ceiling.
    AssetCeilingReached = 5,
    /// The yield rate is outside +/- [`MAX_ABS_RATE_BPS`].
    RateOutOfRange = 6,
    /// A non-admin called an admin-only function.
    Unauthorized = 7,
    /// The contract has not been constructed, or was constructed twice.
    AlreadyInitialized = 8,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Asset,
    TotalAssets,
    TotalShares,
    RateBps,
    LastAccrualTs,
    ShareBalance(Address),
}

/// Topics: `deposit`, the receiver. Data: the two amounts.
#[contractevent]
struct DepositEvent {
    #[topic]
    receiver: Address,
    assets: i128,
    shares: i128,
}

/// Topics: `redeem`, the owner, the receiver. Data: the two amounts.
#[contractevent]
struct RedeemEvent {
    #[topic]
    owner: Address,
    #[topic]
    receiver: Address,
    shares: i128,
    assets: i128,
}

/// Topics: `rate`. Data: the new rate and the asset balance it applies to.
#[contractevent]
struct RateChangedEvent {
    rate_bps: i32,
    total_assets: i128,
}

#[contract]
pub struct MockVault;

#[contractimpl]
impl MockVault {
    /// Builds a vault over `asset`, administered by `admin`, starting with a
    /// yield rate of zero.
    pub fn __constructor(env: Env, admin: Address, asset: Address) {
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        let now = env.ledger().timestamp();
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Asset, &asset);
        env.storage()
            .persistent()
            .set(&DataKey::TotalAssets, &0_i128);
        env.storage()
            .persistent()
            .set(&DataKey::TotalShares, &0_i128);
        env.storage().persistent().set(&DataKey::RateBps, &0_i32);
        env.storage()
            .persistent()
            .set(&DataKey::LastAccrualTs, &now);
    }

    /// Sets the annualised yield rate, in basis points. Negative is allowed.
    ///
    /// Admin only. Takes effect from the current timestamp, so changing the
    /// rate never retroactively re-prices time already served.
    pub fn set_yield_rate(env: Env, rate_bps: i32) {
        load_admin(&env).require_auth();
        if rate_bps.abs() > MAX_ABS_RATE_BPS {
            panic!("rate out of range");
        }
        accrue_internal(&env);
        env.storage().persistent().set(&DataKey::RateBps, &rate_bps);
        RateChangedEvent {
            rate_bps,
            total_assets: total_assets(&env),
        }
        .publish(&env);
    }

    /// The current annualised yield rate in basis points.
    pub fn yield_rate(env: Env) -> i32 {
        env.storage()
            .persistent()
            .get(&DataKey::RateBps)
            .unwrap_or(0)
    }

    /// Forces yield to accrue up to the current timestamp.
    ///
    /// Permissionless on purpose: it only moves the vault's own accounting
    /// forward to the present, it cannot create a gain, and a test or a
    /// keeper should not need admin rights to make the books current.
    pub fn accrue(env: Env) {
        accrue_internal(&env);
    }

    /// The share balance of `account`, after accruing.
    pub fn share_balance(env: Env, account: Address) -> i128 {
        accrue_internal(&env);
        env.storage()
            .persistent()
            .get(&DataKey::ShareBalance(account))
            .unwrap_or(0)
    }

    /// The underlying token, for the dashboard and for epoch-manager's
    /// pinning of the asset at epoch creation.
    pub fn underlying_asset(env: Env) -> Address {
        load_asset(&env)
    }

    /// The administrator.
    pub fn admin(env: Env) -> Address {
        load_admin(&env)
    }
}

#[contractimpl]
impl VaultInterface for MockVault {
    fn asset(env: Env) -> Address {
        load_asset(&env)
    }

    fn deposit(env: Env, assets: i128, receiver: Address) -> i128 {
        if assets <= 0 {
            panic!("zero amount");
        }
        // Accrue first, so the depositor only earns from now on.
        accrue_internal(&env);

        let vault = env.current_contract_address();
        let asset = load_asset(&env);

        // The vault pulls the assets, so the receiver must authorise it. When
        // the receiver is a contract calling us directly, that authorisation
        // comes from the caller's own invocation and needs no extra plumbing.
        receiver.require_auth();
        token::Client::new(&env, &asset).transfer(&receiver, &vault, &assets);

        let shares = convert_to_shares(&env, assets);
        if shares <= 0 {
            // Would mint nothing for a real transfer, so refuse rather than
            // take the assets and issue nothing.
            panic!("zero shares");
        }

        bump_shares(env.storage().persistent(), &receiver, shares);
        let new_assets = total_assets(&env)
            .checked_add(assets)
            .unwrap_or_else(|| panic!("asset ceiling reached"));
        if new_assets > MAX_ASSETS {
            panic!("asset ceiling reached");
        }
        env.storage()
            .persistent()
            .set(&DataKey::TotalAssets, &new_assets);

        DepositEvent {
            receiver,
            assets,
            shares,
        }
        .publish(&env);

        shares
    }

    fn redeem(env: Env, shares: i128, receiver: Address, owner: Address) -> i128 {
        if shares <= 0 {
            panic!("zero amount");
        }
        accrue_internal(&env);

        let owner_balance = share_balance_of(&env, &owner);
        if owner_balance < shares {
            panic!("insufficient shares");
        }

        // ERC-4626: the caller must be authorised by the owner, unless it is
        // the owner. Epoch-manager always redeems its own shares, so it never
        // needs a separate allowance, and this contract never needs one.
        if owner != env.current_contract_address() {
            owner.require_auth();
        }

        let assets = shares_to_assets(&env, shares);

        // Burn unconditionally, even when the vault is worth nothing. The
        // shares were already worthless, and burning them keeps the supply
        // consistent with the burn events and with `total_assets`.
        bump_shares(env.storage().persistent(), &owner, -shares);
        let current = total_assets(&env);
        // `assets` is floored from `shares * total / total_shares`, so it can
        // only exceed the balance by accumulated rounding dust, which is never
        // negative. Saturating subtraction is a guard, not a correctness
        // shortcut.
        let remaining = current.saturating_sub(assets);
        env.storage()
            .persistent()
            .set(&DataKey::TotalAssets, &remaining);

        if assets > 0 {
            let asset = load_asset(&env);
            token::Client::new(&env, &asset).transfer(
                &env.current_contract_address(),
                &receiver,
                &assets,
            );
        }

        RedeemEvent {
            owner,
            receiver,
            shares,
            assets,
        }
        .publish(&env);

        assets
    }

    fn total_assets(env: Env) -> i128 {
        accrue_internal(&env);
        total_assets(&env)
    }

    fn convert_to_assets(env: Env, shares: i128) -> i128 {
        // Reporting must not mutate state, so this reads without accruing.
        // It can therefore be up to one accrual window stale, which is the
        // standard trade-off for a read-only conversion.
        shares_to_assets(&env, shares)
    }

    fn balance_of(env: Env, account: Address) -> i128 {
        share_balance_of(&env, &account)
    }
}

// --- internals -------------------------------------------------------------

fn load_admin(env: &Env) -> Address {
    env.storage()
        .persistent()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic!("not initialized"))
}

fn load_asset(env: &Env) -> Address {
    env.storage()
        .persistent()
        .get(&DataKey::Asset)
        .unwrap_or_else(|| panic!("not initialized"))
}

fn total_assets(env: &Env) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::TotalAssets)
        .unwrap_or(0)
}

fn total_shares(env: &Env) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::TotalShares)
        .unwrap_or(0)
}

fn share_balance_of(env: &Env, account: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::ShareBalance(account.clone()))
        .unwrap_or(0)
}

fn bump_shares(storage: soroban_sdk::storage::Persistent, account: &Address, delta: i128) {
    let key = DataKey::ShareBalance(account.clone());
    let before: i128 = storage.get(&key).unwrap_or(0);
    let total_before = storage.get::<_, i128>(&DataKey::TotalShares).unwrap_or(0);
    let after = before
        .checked_add(delta)
        .unwrap_or_else(|| panic!("arithmetic overflow"));
    if after < 0 {
        panic!("insufficient shares");
    }
    storage.set(&key, &after);
    let total_after = total_before
        .checked_add(delta)
        .unwrap_or_else(|| panic!("arithmetic overflow"));
    storage.set(&DataKey::TotalShares, &total_after);
}

/// The assets `shares` are worth at the current price. Read-only: it does not
/// accrue, so callers that need a settled figure must accrue first.
fn shares_to_assets(env: &Env, shares: i128) -> i128 {
    let assets = total_assets(env);
    let supply = total_shares(env);
    if supply <= 0 || shares <= 0 {
        return 0;
    }
    shares
        .checked_mul(assets)
        .unwrap_or_else(|| panic!("arithmetic overflow"))
        / supply
}

/// The shares `assets` would buy at the current price.
///
/// The order of these two special cases is load-bearing:
///
/// - `supply == 0` is the bootstrap case, checked **first**. On the very first
///   deposit `total_assets` is also zero, so testing the zero-assets case
///   first would hand the first depositor zero shares for a real transfer.
/// - `total_assets == 0` with a live supply is a vault that has been wiped
///   out. Shares still exist but are worth nothing, so a deposit would mint
///   nothing. Returning zero makes the caller refuse rather than take the
///   assets and issue nothing.
fn convert_to_shares(env: &Env, assets: i128) -> i128 {
    let supply = total_shares(env);
    if supply <= 0 {
        // First depositor defines the share price at 1:1.
        return assets;
    }
    let total = total_assets(env);
    if total <= 0 {
        return 0;
    }
    assets
        .checked_mul(supply)
        .unwrap_or_else(|| panic!("arithmetic overflow"))
        / total
}

/// Moves the yield bookkeeping forward to `env.ledger().timestamp()`.
///
/// Named distinctly from the public `accrue` entry point so the internal
/// helper can take `&Env` without shadowing it.
fn accrue_internal(env: &Env) {
    let now = env.ledger().timestamp();
    let last: u64 = env
        .storage()
        .persistent()
        .get(&DataKey::LastAccrualTs)
        .unwrap_or(now);
    if now <= last {
        return;
    }
    let elapsed = (now - last).min(MAX_ACCRUAL_SECONDS);

    let rate: i32 = env
        .storage()
        .persistent()
        .get(&DataKey::RateBps)
        .unwrap_or(0);
    let assets = total_assets(env);

    if rate == 0 || assets == 0 {
        env.storage()
            .persistent()
            .set(&DataKey::LastAccrualTs, &now);
        return;
    }

    let delta = assets
        .checked_mul(rate as i128)
        .and_then(|p| p.checked_mul(elapsed as i128))
        .unwrap_or_else(|| panic!("arithmetic overflow"))
        / YEAR_DENOMINATOR;

    // Clamp at both ends. Zero is the floor because a vault cannot hold a
    // negative balance, and MAX_ASSETS is the ceiling that keeps the accrual
    // product inside i128 forever.
    let next = assets.saturating_add(delta).clamp(0, MAX_ASSETS);
    env.storage().persistent().set(&DataKey::TotalAssets, &next);
    // Set to `now`, not `last + elapsed`, so a clamped window is dropped
    // rather than re-accrued on every subsequent call.
    env.storage()
        .persistent()
        .set(&DataKey::LastAccrualTs, &now);
}

#[cfg(test)]
mod test;
