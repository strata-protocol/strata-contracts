//! Test-only package for Strata.
//!
//! Nothing in here is a contract. It exists to hold the two kinds of test that
//! cannot live in the contract crates themselves:
//!
//! - [`waterfall_props`] — property tests for the five invariants in
//!   [`docs/waterfall-spec.md`](../docs/waterfall-spec.md). They live here
//!   rather than in `contracts/waterfall` so the whole proof surface for the
//!   spec sits in one place, greppable by invariant number.
//! - [`blend_adapter_props`] — property tests for the Blend adapter's rounding
//!   and share ledger, against the pool double.
//! - [`integration`] — cross-contract tests that drive epoch-manager, mock-vault
//!   and a real SEP-41 token through full epoch lifecycles.
//! - [`blend_adapter_integration`] — the same for an epoch-manager whose vault
//!   is the Blend adapter rather than the mock vault.

#![cfg(test)]

pub mod waterfall_props;

#[cfg(test)]
pub mod integration;

pub mod blend_adapter_props;

#[cfg(test)]
pub mod blend_adapter_integration;
#[cfg(test)]
mod hostile_vault_test;
