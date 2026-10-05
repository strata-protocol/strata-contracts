#!/usr/bin/env bash

# Strata TTL upkeep — TESTNET ONLY. UNAUDITED.
#
# > **STATUS: VERIFIED against Stellar testnet on 2026-10-05**, Stellar CLI
# > 28.1.0. It extended all four entries. Measured, at ledger 5 036 514:
# >
# > | entry | ttl before | ttl after |
# > | --- | --- | --- |
# > | vault instance | 5 536 446 | 5 536 519 |
# > | vault wasm | 5 536 464 | 5 536 529 |
# > | manager instance | 5 156 272 | 5 536 540 |
# > | manager wasm | 5 156 267 | 5 536 551 |
# >
# > The network clamps the result: asking for 500 000 ledgers yields about
# > 500 026 ledgers of remaining life, not 500 000 more than whatever was there.
# > The vault rows barely move because an earlier partial run had already taken
# > them to the ceiling. That is the clamp, not a failure.
#
# "Verified" means this script ran to completion on testnet. It does not mean the
# contracts are audited. They are not.
#
# Extends the time-to-live of the deployed contracts and their Wasm so the
# deployment is still readable when a reviewer looks at it.
#
# Usage:
#   ./scripts/extend-ttl-testnet.sh [network] [--ledgers N]
#
# # This is NOT the TTL audit.
#
# It extends exactly two things per contract: the contract instance and its Wasm.
# It does **not** extend the contract's own storage entries, and the reason is a
# real limitation rather than an oversight — see the "why Config, Epoch and
# Position are not extended here" section at the bottom. Risk R9 in
# docs/risks.md remains open and this script does not discharge it.
#
# TTL only moves when an entry is *touched*. The contracts bump on every write,
# so an epoch that keeps transacting keeps itself alive and this script is a
# backstop for an idle deployment, not the mechanism.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# ~29 days at testnet's 5-second ledger cadence. The network clamps this to its
# own max_entry_ttl if the request is larger, so overshooting is safe and
# undershooting silently expires the entry.
LEDGERS=500000

NETWORK="${1:-testnet}"
if [ "${1:-}" = "--ledgers" ]; then
    NETWORK="testnet"
    LEDGERS="$2"
elif [ "${2:-}" = "--ledgers" ]; then
    LEDGERS="$3"
fi

ADMIN_IDENTITY="strata-deployer"

say() { printf '%s\n' "$*"; }
step() { say ""; say "==> $*"; }
die() {
    say "error: $*" >&2
    exit 1
}

[ "$NETWORK" = "testnet" ] ||
    die "network '$NETWORK' is not allowed. This script is testnet-only; Strata is
unaudited and this project has no mainnet code path."

[ -n "$ADMIN_IDENTITY" ] || die "no admin identity configured"
command -v stellar >/dev/null 2>&1 || die "stellar CLI not found"
stellar keys public-key "$ADMIN_IDENTITY" >/dev/null 2>&1 ||
    die "no identity '$ADMIN_IDENTITY'"

RECORD="deployments/$NETWORK.json"
[ -f "$RECORD" ] || die "$RECORD not found. Run scripts/deploy-testnet.sh first."

# Pull a field out of the record's contracts section, by key.
#
# All three strips are needed: the greedy `.*"://` leaves the whitespace and
# opening quote of the value behind, and only stripping the closing quote would
# hand the CLI a "contract id" with a stray `"` in front of it.
record_contract() {
    grep -A 8 "\"$1\":" "$RECORD" | grep -oE "\"$2\": *\"[^\"]+\"" | head -n 1 |
        sed 's/.*"://; s/^[[:space:]]*"//; s/"[[:space:]]*$//'
}

VAULT_ID=$(record_contract mock_vault contract_id)
VAULT_WASM=$(record_contract mock_vault wasm_sha256)
MANAGER_ID=$(record_contract epoch_manager contract_id)
MANAGER_WASM=$(record_contract epoch_manager wasm_sha256)

[ -n "$VAULT_ID" ] && [ -n "$MANAGER_ID" ] && [ -n "$VAULT_WASM" ] && [ -n "$MANAGER_WASM" ] ||
    die "could not read the contract ids and wasm hashes out of $RECORD"

say "Strata TTL upkeep on $NETWORK by $LEDGERS ledgers. UNAUDITED."
say "  vault   $VAULT_ID"
say "  manager $MANAGER_ID"
say "  admin   $(stellar keys public-key "$ADMIN_IDENTITY")"

# Report an entry's current TTL ledger without changing it. `--ttl-ledger-only`
# still submits a (no-op) transaction, so this is a network call, not a free read,
# but it does not move the TTL.
ttl_of() {
    stellar contract extend \
        --source-account "$ADMIN_IDENTITY" \
        --network "$NETWORK" \
        --no-cache \
        --ttl-ledger-only \
        --ledgers-to-extend 1 \
        "$@" 2>&1 | grep -oE '^[0-9]+$' | tail -n 1 || true
}

extend() {
    local label="$1"
    shift
    local before after out
    before=$(ttl_of "$@")
    say "  $label"
    say "    ttl before: ${before:-unknown}"
    if ! out=$(stellar contract extend \
        --source-account "$ADMIN_IDENTITY" \
        --network "$NETWORK" \
        --no-cache \
        --ledgers-to-extend "$LEDGERS" \
        "$@" 2>&1); then
        printf '%s\n' "$out" >&2
        die "extending $label failed"
    fi
    after=$(ttl_of "$@")
    say "    ttl after:  ${after:-unknown}"
    if [ -n "$before" ] && [ -n "$after" ] && [ "$after" -le "$before" ]; then
        die "$label did not gain TTL: $before -> $after.
The entry is probably already at the network's max_entry_ttl, in which case this
is expected and not an error."
    fi
}

LATEST=$(stellar ledger latest --network "$NETWORK" 2>&1 | sed -n 's/^Sequence: //p' | head -n 1)
step "latest ledger is ${LATEST:-unknown}"

for pair in "vault:$VAULT_ID:$VAULT_WASM" "manager:$MANAGER_ID:$MANAGER_WASM"; do
    name="${pair%%:*}"
    rest="${pair#*:}"
    id="${rest%%:*}"
    wasm="${rest#*:}"

    step "$name contract instance"
    extend "$name instance" --contract-id "$id"

    step "$name contract Wasm"
    extend "$name wasm" --wasm-hash "$wasm"
done

step "why Config, Epoch and Position are not extended here"
say "  epoch-manager stores its state under a #[contracttype] enum, DataKey, so the"
say "  entries are composite ScVals rather than bare symbols. 'stellar contract"
say "  extend --key' only builds symbol keys, so it cannot address Config, Epoch or"
say "  Position(address, tranche) at all — it reports 'Ledger entry not found' for"
say "  Config. Reaching them externally needs the XDR encoding of each key, which"
say "  this script does not attempt to guess."
say ""
say "  It does not need to. The contract bumps those entries itself: every write"
say "  calls extend_ttl with BUMP_THRESHOLD 100_000 and BUMP_AMOUNT 4_000_000"
say "  ledgers (see BUMP_THRESHOLD/BUMP_AMOUNT in contracts/epoch-manager). That is"
say "  eight times what this script extends the instance by, and it happens on"
say "  every lifecycle transition and every deposit and claim."
say ""
say "  The gap is a *depositor who deposits and then never claims*: their position"
say "  is bumped at deposit time and then left alone. That is risk R9 in"
say "  docs/risks.md, it is still open, and this script does not discharge it."

step "done"
say "Extended each contract instance and its Wasm by $LEDGERS ledgers (the network"
say "clamps the result to its own maximum)."
say ""
say "Unaudited testnet software. Do not use real funds."