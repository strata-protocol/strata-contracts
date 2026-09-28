//! The subset of ERC-4626 that Strata requires of an underlying vault.
//!
//! This crate has no logic. It declares the interface trait, and the
//! `#[contractclient]` attribute generates a `VaultClient` that epoch-manager
//! uses to call into whatever vault it wraps. Splitting the interface into its
//! own crate is what lets the manager depend on the interface without
//! depending on a particular implementation, which is the whole point of
//! "wraps any ERC-4626-style vault".
//!
//! # The four required functions
//!
//! The product brief names `deposit`, `redeem`, `total_assets` and
//! `convert_to_assets`. This interface is those four plus two additions, both
//! forced by the settlement path and both documented below.
//!
//! ## Additions
//!
//! - **`asset()`** — the epoch manager reads the underlying token address once,
//!   at `create_epoch`, and stores it. Pinning it there means a vault cannot
//!   change which token it holds partway through an epoch and strand the
//!   manager's funds.
//! - **`balance_of(account)`** — needed to read the manager's share balance at
//!   redemption. Without it the manager would have to trust the `shares`
//!   value returned by its own `deposit` call and could never verify that
//!   everything came back. A "nothing left behind" check is worth more than
//!   the two extra lines.
//!
//! Both are part of standard ERC-4626, so this remains a subset of a
//! well-defined interface rather than a Strata invention.
//!
//! # Deliberately excluded
//!
//! `mint`, `withdraw`, `convert_to_shares`, `max_deposit`, `max_mint`,
//! `max_withdraw`, `max_redeem`, and the ERC-20 allowance surface. Strata
//! deposits assets and redeems shares, never the reverse, and it never relies
//! on allowance-based pulls. Leaving them out keeps the surface an
//! implementation must satisfy small.
//!
//! The SEP-41 token interface itself comes from `soroban_sdk::token`, which is
//! a host function rather than a contract, so it is not re-declared here.

#![no_std]

use soroban_sdk::{contractclient, Address, Env};

/// The vault surface Strata depends on.
///
/// Implemented by `strata-mock-vault` and expected of any real vault wrapped
/// by epoch-manager. See the crate docs for what is included and why.
///
/// Note the `Env` first argument: `#[contractclient]` interface traits take it
/// explicitly. The generated `VaultClient` hides it, so calls read as
/// `client.deposit(&assets, &receiver)`.
#[contractclient(name = "VaultClient")]
pub trait VaultInterface {
    /// The underlying token this vault holds.
    ///
    /// Read once at epoch creation and pinned, so the asset cannot change
    /// mid-epoch.
    fn asset(env: Env) -> Address;

    /// Deposits `assets` and mints shares to `receiver`.
    ///
    /// Returns the shares minted. Strata records this so it can account for
    /// its position, and re-reads `balance_of` at redemption to check.
    fn deposit(env: Env, assets: i128, receiver: Address) -> i128;

    /// Burns exactly `shares` from `owner` and sends the assets to `receiver`.
    ///
    /// Shares-proportional rather than asset-denominated, because the share
    /// price moves over the epoch and Strata wants all of its position, not a
    /// fixed asset amount.
    fn redeem(env: Env, shares: i128, receiver: Address, owner: Address) -> i128;

    /// Total assets currently managed by the vault, including accrued yield.
    fn total_assets(env: Env) -> i128;

    /// The asset value of `shares`, at the current share price.
    ///
    /// Reported for depositors to see their position. Strata's settlement
    /// uses the amount actually returned by `redeem`, never this, because a
    /// vault could report one thing and transfer another.
    fn convert_to_assets(env: Env, shares: i128) -> i128;

    /// The share balance of `account`.
    fn balance_of(env: Env, account: Address) -> i128;
}
