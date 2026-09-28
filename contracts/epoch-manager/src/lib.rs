//! Fixed-term senior/junior epochs over a single underlying ERC-4626 vault.
//!
//! Strata opens one epoch at a time. During an epoch, depositors choose a
//! tranche. At maturity anyone can settle, which redeems the manager's entire
//! vault position and splits the result with
//! [`strata_waterfall`](strata_waterfall) — the pure, heavily property-tested
//! settlement math. Afterwards each depositor claims their pro-rata share.
//!
//! This contract owns the money and the state machine. It owns none of the
//! arithmetic: every payout figure comes from the waterfall crate, so there is
//! exactly one implementation of the split to audit.
//!
//! # What is deliberately not here
//!
//! - **Fees.** None, of any kind.
//! - **Concurrent epochs.** One active epoch at a time, decided at design
//!   time. It keeps the state machine small enough to audit, which matters
//!   more for an unaudited contract than epoch flexibility.
//! - **Early exit or transfer.** A tranche position is locked for the term.
//!   Positions are not transferable, so there is no allowance surface to
//!   reason about.
//! - **Re-opening.** A settled epoch is immutable. The only forward path is
//!   `close_epoch`, which requires every claim to have been made.
//!
//! # Lifecycle
//!
//! ```text
//! (no epoch) --create_epoch--> Open --settle--> Settled --close_epoch--> (no epoch)
//!                deposits          mature         claims            all claimed
//! ```
//!
//! Deposits close automatically at maturity, with no extra call: `deposit`
//! simply refuses once `now >= maturity_ts`.
//!
//! See [`docs/architecture.md`](../../docs/architecture.md) for the storage
//! layout and the auth model, and
//! [`docs/waterfall-spec.md`](../../docs/waterfall-spec.md) for the split.

#![no_std]

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractevent, contractimpl, contracttype, symbol_short, vec, Address,
    Env, IntoVal, Symbol,
};
use strata_vault_interface::VaultClient;
use strata_waterfall::{check_rate, check_ratio, check_term, validate_principal, WaterfallError};

/// Minimum remaining life before a storage entry is worth extending, in
/// ledgers.
const BUMP_THRESHOLD: u32 = 100_000;
/// How much life to add, in ledgers. Generous relative to a one-year maximum
/// term: the cost of an entry outliving its usefulness is a little rent, and
/// the cost of losing a depositor's position is their money.
const BUMP_AMOUNT: u32 = 4_000_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// The contract is already initialised. A constructor cannot be re-run.
    AlreadyInitialized = 1,
    /// An epoch is already open, or is settled and not yet fully claimed.
    EpochAlreadyActive = 2,
    /// No epoch exists.
    NoActiveEpoch = 3,
    /// The epoch has not reached maturity, so it cannot be settled yet.
    NotMature = 4,
    /// The epoch is already settled.
    AlreadySettled = 5,
    /// A deposit arrived at or after maturity.
    DepositsClosed = 6,
    /// Only the admin may do this.
    Unauthorized = 7,
    /// An amount was zero or negative.
    InvalidAmount = 8,
    /// An input violated a documented limit in the waterfall spec.
    InvalidParameter = 9,
    /// A senior deposit would breach the junior buffer gate. See spec
    /// section 7.
    SeniorGateViolated = 10,
    /// The caller holds no position in that tranche.
    NoPosition = 11,
    /// Arithmetic exceeded the documented bounds.
    ArithmeticOverflow = 12,
    /// The epoch still has unclaimed positions.
    ClaimsOutstanding = 13,
    /// The manager held no vault shares at settlement.
    NoVaultShares = 14,
}

/// Which tranche a position belongs to.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tranche {
    /// Capped at a fixed target rate, paid first, protected by the junior
    /// buffer.
    Senior,
    /// Receives everything above the senior target, absorbs losses first.
    Junior,
}

/// Where an epoch is in its lifecycle.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Accepting deposits, not yet at maturity.
    Open,
    /// Past maturity and settled. Payouts are fixed; claims are open.
    Settled,
    /// Every position has been claimed. A new epoch may be created.
    Closed,
}

/// Everything about the current epoch, in one entry.
///
/// Kept as a single struct so one TTL bump covers the whole epoch.
/// Per-depositor positions are separate entries, bumped individually.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epoch {
    /// Ledger timestamp at which the epoch opened.
    pub start_ts: u64,
    /// Ledger timestamp at which the epoch matures. Deposits close here.
    pub maturity_ts: u64,
    /// `maturity_ts - start_ts`, the waterfall's `t`.
    pub term_seconds: u64,
    /// Senior target rate in basis points per year.
    pub rate_bps: u32,
    /// Cap on senior principal as a fraction of junior principal, in bps.
    pub max_senior_ratio_bps: u32,
    /// Total senior principal deposited.
    pub senior_total: i128,
    /// Total junior principal deposited.
    pub junior_total: i128,
    /// Sum of all outstanding senior shares, to detect the last claim.
    pub senior_shares_outstanding: i128,
    /// Sum of all outstanding junior shares.
    pub junior_shares_outstanding: i128,
    /// Vault shares held by the manager, redeemed at settlement.
    pub vault_shares: i128,
    /// Assets actually redeemed from the vault: the spec's `V`.
    pub value_redeemed: i128,
    /// The senior's share of `V`, fixed at settlement.
    pub senior_payout: i128,
    /// The junior's share of `V`, fixed at settlement.
    pub junior_payout: i128,
    /// Senior already paid out, for the last-claimer's remainder.
    pub senior_paid: i128,
    /// Junior already paid out.
    pub junior_paid: i128,
    /// Lifecycle position.
    pub status: Status,
}

#[contracttype]
pub enum DataKey {
    Config,
    Epoch,
    Position(Address, Tranche),
}

/// Addresses the contract was built with. Immutable after construction.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub admin: Address,
    /// The underlying ERC-4626 vault.
    pub vault: Address,
    /// The underlying token, read from the vault once at construction and
    /// pinned. Re-reading per call would let a vault swap the token out from
    /// under a live epoch.
    pub asset: Address,
}

/// A depositor's position in one tranche.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    /// The depositor's pro-rata share of the tranche's principal.
    pub shares: i128,
    /// What those shares are worth. Exact once the epoch settles; a projection
    /// against the vault's live valuation before that.
    pub estimated_payout: i128,
}

/// The result of projecting an arbitrary `V` through the waterfall.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedSplit {
    /// The senior's target for this epoch's terms and principal.
    pub senior_due: i128,
    /// What the senior would receive.
    pub senior_payout: i128,
    /// What the junior would receive.
    pub junior_payout: i128,
}

#[contractevent]
struct InitializedEvent {
    #[topic]
    admin: Address,
    #[topic]
    vault: Address,
    asset: Address,
}

#[contractevent]
struct EpochOpenedEvent {
    start_ts: u64,
    maturity_ts: u64,
    term_seconds: u64,
    rate_bps: u32,
    max_senior_ratio_bps: u32,
}

#[contractevent]
struct EpochClosedEvent {
    #[topic]
    epoch_start_ts: u64,
    value_redeemed: i128,
}

#[contract]
pub struct EpochManager;

#[contractimpl]
impl EpochManager {
    /// Builds a manager over `vault`, administered by `admin`.
    ///
    /// The underlying token is read from the vault here and pinned, so it
    /// cannot change underneath a live epoch. A vault whose `asset()` call
    /// fails cannot be wrapped, which is the intended behaviour.
    pub fn __constructor(env: Env, admin: Address, vault: Address) {
        if env.storage().persistent().has(&DataKey::Config) {
            panic!("already initialized");
        }
        let asset = VaultClient::new(&env, &vault).asset();

        env.storage().persistent().set(
            &DataKey::Config,
            &Config {
                admin: admin.clone(),
                vault: vault.clone(),
                asset: asset.clone(),
            },
        );
        // No epoch key is written. Its absence *is* "no epoch"; see
        // `load_epoch`.
        bump(&env, &DataKey::Config);

        InitializedEvent {
            admin,
            vault,
            asset,
        }
        .publish(&env);
    }

    // --- epoch lifecycle ---

    /// Opens a new epoch. Admin only.
    ///
    /// Refuses while an epoch is open, or while a settled epoch still has
    /// unclaimed positions. That is what "one active epoch at a time" means in
    /// code: there is no path to a second epoch while funds from the first are
    /// still outstanding.
    ///
    /// Every parameter is validated against the documented limits in the
    /// waterfall spec *before* anything is written, so a rejected epoch leaves
    /// no state behind.
    pub fn create_epoch(
        env: Env,
        term_seconds: u64,
        rate_bps: u32,
        max_senior_ratio_bps: u32,
    ) -> Result<(), Error> {
        let config = load_config(&env);
        config.admin.require_auth();

        match load_epoch(&env) {
            None => {}
            Some(ref e) if e.status == Status::Closed => {}
            Some(_) => return Err(Error::EpochAlreadyActive),
        }

        check_term(term_seconds).map_err(map_waterfall)?;
        check_rate(rate_bps).map_err(map_waterfall)?;
        check_ratio(max_senior_ratio_bps).map_err(map_waterfall)?;

        let start_ts = env.ledger().timestamp();
        // Saturating, not wrapping. An overflow here would produce a maturity
        // timestamp in the past and silently skip the whole term.
        let maturity_ts = start_ts
            .checked_add(term_seconds)
            .ok_or(Error::InvalidParameter)?;

        store_epoch(
            &env,
            &Epoch {
                start_ts,
                maturity_ts,
                term_seconds,
                rate_bps,
                max_senior_ratio_bps,
                senior_total: 0,
                junior_total: 0,
                senior_shares_outstanding: 0,
                junior_shares_outstanding: 0,
                vault_shares: 0,
                value_redeemed: 0,
                senior_payout: 0,
                junior_payout: 0,
                senior_paid: 0,
                junior_paid: 0,
                status: Status::Open,
            },
        );

        EpochOpenedEvent {
            start_ts,
            maturity_ts,
            term_seconds,
            rate_bps,
            max_senior_ratio_bps,
        }
        .publish(&env);

        Ok(())
    }

    /// Closes a fully-claimed settled epoch, freeing the manager to open
    /// another.
    ///
    /// Permissionless on purpose: it can only move the state machine forward,
    /// and only once nothing is owed to anyone.
    pub fn close_epoch(env: Env) -> Result<(), Error> {
        let mut epoch = load_epoch(&env).ok_or(Error::NoActiveEpoch)?;
        if epoch.status != Status::Settled {
            return Err(Error::NotMature);
        }
        if epoch.senior_shares_outstanding > 0 || epoch.junior_shares_outstanding > 0 {
            return Err(Error::ClaimsOutstanding);
        }
        epoch.status = Status::Closed;
        store_epoch(&env, &epoch);

        EpochClosedEvent {
            epoch_start_ts: epoch.start_ts,
            value_redeemed: epoch.value_redeemed,
        }
        .publish(&env);

        Ok(())
    }

    // --- views ---

    /// The administrator.
    pub fn admin(env: Env) -> Address {
        load_config(&env).admin
    }

    /// The underlying ERC-4626 vault.
    pub fn vault(env: Env) -> Address {
        load_config(&env).vault
    }

    /// The underlying token, pinned at construction.
    pub fn asset(env: Env) -> Address {
        load_config(&env).asset
    }

    /// The current epoch, or nothing if none is open.
    pub fn current_epoch(env: Env) -> Option<Epoch> {
        load_epoch(&env)
    }

    /// A depositor's position in one tranche, and what it is currently worth.
    pub fn position_of(env: Env, who: Address, tranche: Tranche) -> Result<Position, Error> {
        let epoch = load_epoch(&env).ok_or(Error::NoActiveEpoch)?;
        let shares = load_position(&env, &who, tranche);

        let (tranche_payout, tranche_total) = match tranche {
            Tranche::Senior => (epoch.senior_payout, epoch.senior_total),
            Tranche::Junior => (epoch.junior_payout, epoch.junior_total),
        };

        // Before settlement there is no `V`, so the figure is a projection
        // against the vault's live valuation. It becomes exact at settlement.
        let estimated_payout = if epoch.status == Status::Open {
            project_position(&env, &epoch, tranche, shares)
        } else if tranche_total <= 0 {
            0
        } else {
            mul_div(tranche_payout, shares, tranche_total)?
        };

        Ok(Position {
            shares,
            estimated_payout,
        })
    }

    /// How much senior principal the gate would still admit, given the junior
    /// principal deposited so far. Zero once deposits have closed.
    pub fn senior_room(env: Env) -> Result<i128, Error> {
        let epoch = load_epoch(&env).ok_or(Error::NoActiveEpoch)?;
        if epoch.status != Status::Open {
            return Ok(0);
        }
        Ok(
            strata_waterfall::max_senior_total(epoch.junior_total, epoch.max_senior_ratio_bps)
                .unwrap_or(0),
        )
    }

    /// Seconds until maturity, or zero once it has passed.
    pub fn seconds_to_maturity(env: Env) -> Result<u64, Error> {
        let epoch = load_epoch(&env).ok_or(Error::NoActiveEpoch)?;
        // Saturating: past maturity the remainder would be negative, and a
        // view must not trap on a clock that has simply moved on.
        Ok(epoch.maturity_ts.saturating_sub(env.ledger().timestamp()))
    }

    /// What the two tranches would receive if the vault were worth `value`
    /// right now.
    ///
    /// The dashboard's projection hook: the waterfall applied to an arbitrary
    /// `V`, so a depositor can see their outcome across a range of underlying
    /// returns without waiting for maturity.
    pub fn project(env: Env, value: i128) -> Result<ProjectedSplit, Error> {
        let epoch = load_epoch(&env).ok_or(Error::NoActiveEpoch)?;
        let s = strata_waterfall::settle(
            epoch.senior_total,
            epoch.junior_total,
            epoch.rate_bps,
            epoch.term_seconds,
            value,
        )
        .map_err(map_waterfall)?;

        Ok(ProjectedSplit {
            senior_due: s.senior_due,
            senior_payout: s.senior_payout,
            junior_payout: s.junior_payout,
        })
    }

    /// The manager's vault shares, for accounting and for tests.
    pub fn vault_shares(env: Env) -> i128 {
        load_epoch(&env).map_or(0, |e| e.vault_shares)
    }
}

// --- storage helpers ---

fn load_config(env: &Env) -> Config {
    env.storage()
        .persistent()
        .get(&DataKey::Config)
        .unwrap_or_else(|| panic!("not initialized"))
}

fn load_epoch(env: &Env) -> Option<Epoch> {
    // Presence of the key is what means "an epoch exists". Storing an
    // `Option<Epoch>` and reading it back is ambiguous: `Storage::get` returns
    // `Option<V>`, so a `-> Option<Epoch>` signature silently infers
    // `V = Epoch` and then fails to decode a stored `None`. Checking `has`
    // first makes the intent explicit and removes the inference trap.
    if !env.storage().persistent().has(&DataKey::Epoch) {
        return None;
    }
    env.storage().persistent().get(&DataKey::Epoch)
}

fn store_epoch(env: &Env, epoch: &Epoch) {
    env.storage().persistent().set(&DataKey::Epoch, epoch);
    bump(env, &DataKey::Epoch);
}

fn load_position(env: &Env, who: &Address, tranche: Tranche) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Position(who.clone(), tranche))
        .unwrap_or(0)
}

/// Extends an entry's time-to-live. Silently does nothing if the entry has
/// already been restored or removed, which is the right behaviour: TTL upkeep
/// must never be the thing that fails a deposit.
fn bump(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, BUMP_THRESHOLD, BUMP_AMOUNT);
}

/// `a * b / c`, with a trap rather than a wrap on overflow.
fn mul_div(a: i128, b: i128, c: i128) -> Result<i128, Error> {
    if c <= 0 {
        return Err(Error::ArithmeticOverflow);
    }
    a.checked_mul(b)
        .map(|p| p / c)
        .ok_or(Error::ArithmeticOverflow)
}

/// Projects a depositor's shares against the vault's live valuation.
fn project_position(env: &Env, epoch: &Epoch, tranche: Tranche, shares: i128) -> i128 {
    if shares <= 0 || epoch.vault_shares <= 0 {
        return 0;
    }
    let config = load_config(env);
    let value = VaultClient::new(env, &config.vault).convert_to_assets(&epoch.vault_shares);
    let settlement = match strata_waterfall::settle(
        epoch.senior_total,
        epoch.junior_total,
        epoch.rate_bps,
        epoch.term_seconds,
        value,
    ) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let (tranche_payout, tranche_total) = match tranche {
        Tranche::Senior => (settlement.senior_payout, epoch.senior_total),
        Tranche::Junior => (settlement.junior_payout, epoch.junior_total),
    };
    if tranche_total <= 0 {
        return 0;
    }
    mul_div(tranche_payout, shares, tranche_total).unwrap_or(0)
}

/// Maps a waterfall error onto the contract's error enum.
///
/// The variants line up one for one, so a rejection points at a specific
/// clause of the spec rather than a generic failure.
pub fn map_waterfall(err: WaterfallError) -> Error {
    match err {
        WaterfallError::NegativeAmount => Error::InvalidAmount,
        WaterfallError::PrincipalTooLarge
        | WaterfallError::RateTooHigh
        | WaterfallError::TermOutOfRange
        | WaterfallError::RatioOutOfRange
        | WaterfallError::Overflow => Error::InvalidParameter,
        WaterfallError::SeniorGateViolated => Error::SeniorGateViolated,
    }
}

/// Lifts a `WaterfallError` into a `soroban_sdk::Error` carrying our enum's
/// code.
///
/// Relies on the `Error` enum declaring its discriminants explicitly, which it
/// does, so the enum value *is* the on-chain error code.
/// `error_codes_match_discriminants` in the tests pins that.
pub fn wf(err: WaterfallError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(map_waterfall(err) as u32)
}

/// Rejects a non-positive or over-ceiling amount.
pub fn check_amount(amount: i128) -> Result<(), Error> {
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    validate_principal(amount).map_err(map_waterfall)
}

/// The function name a token `transfer` is authorised under.
pub const TRANSFER_FN: Symbol = symbol_short!("transfer");

/// Builds the authorisation entry that lets the manager's own tokens be
/// pulled by the vault.
///
/// The manager calls the vault directly, and the vault then calls the token
/// contract. That token call sits two frames below the manager, so the
/// manager's authorisation for it must be declared up front with
/// `authorize_as_current_contract`. Without the entry the call is rejected on
/// chain — which is the right failure mode, because a missing entry should stop
/// a deposit rather than allow an unauthorised pull.
pub fn transfer_auth_entry(
    env: &Env,
    asset: &Address,
    vault: &Address,
    assets: i128,
) -> InvokerContractAuthEntry {
    let manager = env.current_contract_address();
    InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: asset.clone(),
            fn_name: TRANSFER_FN,
            args: vec![
                env,
                manager.into_val(env),
                vault.clone().into_val(env),
                assets.into_val(env),
            ],
        },
        sub_invocations: vec![env],
    })
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test;
