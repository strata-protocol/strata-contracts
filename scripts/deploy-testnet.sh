# Strata deploy — TESTNET ONLY.
#
# > **STATUS: UNVERIFIED.** This script has never been executed against a live
# > network, because doing so needs funded testnet credentials that this
# > repository does not have. Treat it as untested code. See the status table
# > in the README. The network guard below *is* exercised, by `deploy-testnet.sh
# > --self-test`.
#
# Refuses to run against anything that is not a recognised testnet. This is
# the control behind the "testnet only" rule in the README; the rule itself is
# not a control.
#
# Usage:
#   ./scripts/deploy-testnet.sh <path-to-stellar-identity-file> [network]
#   ./scripts/deploy-testnet.sh --self-test
#
# Defaults to `testnet`. There is no mainnet code path, by design.

set -euo pipefail

# Networks this project is allowed to touch. Adding a real one is a
# deliberate, reviewed change to this list — not a flag someone flips.
ALLOWED_NETWORKS=("testnet" "local" "futurenet")

die() {
    echo "error: $*" >&2
    exit 1
}

# The one part of this script that can be checked without a network. Kept
# separate so the mainnet refusal is actually tested rather than assumed.
assert_network_is_allowed() {
    local network="$1"
    for candidate in "${ALLOWED_NETWORKS[@]}"; do
        if [ "$network" = "$candidate" ]; then
            return 0
        fi
    done
    return 1
}

if [ "${1:-}" = "--self-test" ]; then
    for good in "${ALLOWED_NETWORKS[@]}"; do
        assert_network_is_allowed "$good" || { echo "FAIL: rejected $good"; exit 1; }
        echo "ok: $good allowed"
    done
    for bad in mainnet public "Main Net" "" futurenetX; do
        if assert_network_is_allowed "$bad"; then
            echo "FAIL: accepted '$bad'"
            exit 1
        fi
        echo "ok: '$bad' refused"
    done
    echo "network guard behaves correctly"
    exit 0
fi

IDENTITY="${1:-}"
NETWORK="${2:-testnet}"

# The network guard is checked first, before anything else, so that asking for
# mainnet fails on the network no matter what else is wrong with the
# invocation. A guard that runs fifth is a guard that reports the wrong reason.
assert_network_is_allowed "$NETWORK" || die "network '$NETWORK' is not allowed.
Strata is unaudited and testnet-only. Allowed: ${ALLOWED_NETWORKS[*]}
If you believe this should be different, that is a discussion for an issue,
not an edit to this script."

[ -n "$IDENTITY" ] || die "usage: $0 <identity-file> [network]"
[ -f "$IDENTITY" ] || die "identity file not found: $IDENTITY"

command -v stellar >/dev/null 2>&1 ||
    die "stellar CLI not found. Install it with: cargo install stellar-cli --locked"

# Build first, and fail loudly if it does not produce deployable Wasm. A plain
# `cargo build` produces nothing the Soroban runtime will accept: the runtime
# requires wasm32v1-none built by `stellar contract build`.
echo "==> building contracts"
stellar contract build

WASM_DIR="target/wasm32v1-none/release"

# Discovered by glob rather than hardcoded. `stellar contract build` names the
# output after the contract spec, which is not guaranteed to match the crate
# name, and hardcoding a filename here would break on an SDK upgrade. Any
# single wasm containing the marker wins.
find_wasm() {
    local marker="$1"
    local found=()
    local candidate
    for candidate in "$WASM_DIR"/*"$marker"*.wasm; do
        [ -f "$candidate" ] && found+=("$candidate")
    done
    if [ "${#found[@]}" -eq 0 ]; then
        return 1
    fi
    printf '%s\n' "${found[0]}"
}

MANAGER_WASM=$(find_wasm epoch_manager) ||
    die "no wasm matching *epoch_manager* in $WASM_DIR. Did 'stellar contract build' run?"
VAULT_WASM=$(find_wasm mock_vault) ||
    die "no wasm matching *mock_vault* in $WASM_DIR. Did 'stellar contract build' run?"

for wasm in "$MANAGER_WASM" "$VAULT_WASM"; do
    echo "==> $wasm ($(wc -c <"$wasm" | tr -d ' ') bytes)"
done

ADMIN=$(stellar keys address --public-key "$IDENTITY")
echo "==> admin: $ADMIN"

# The asset has to exist before the vault that wraps it, because the vault
# takes the token address in its constructor.
echo "==> creating the underlying asset"
ASSET_ID=$(stellar contract create asset \
    --issuer "$ADMIN" \
    --code 3 \
    --name "Strata Test USD" \
    --symbol TUSD \
    --decimals 7 \
    --network "$NETWORK" | tail -n 1)
[ -n "$ASSET_ID" ] || die "could not determine the asset id"
echo "    asset: $ASSET_ID"

echo "==> deploying mock-vault to $NETWORK"
VAULT_ID=$(stellar contract deploy \
    --wasm "$VAULT_WASM" \
    --source-account "$IDENTITY" \
    --network "$NETWORK" \
    --alias strata-mock-vault \
    -- \
    --admin "$ADMIN" \
    --asset "$ASSET_ID" | tail -n 1)
[ -n "$VAULT_ID" ] || die "could not determine the vault id"
echo "    vault: $VAULT_ID"

echo "==> deploying epoch-manager to $NETWORK"
MANAGER_ID=$(stellar contract deploy \
    --wasm "$MANAGER_WASM" \
    --source-account "$IDENTITY" \
    --network "$NETWORK" \
    --alias strata-epoch-manager \
    -- \
    --admin "$ADMIN" \
    --vault "$VAULT_ID" | tail -n 1)
[ -n "$MANAGER_ID" ] || die "could not determine the manager id"
echo "    manager: $MANAGER_ID"

cat <<EOF

Deployed to $NETWORK. UNAUDITED - do not use real funds.

  vault   $VAULT_ID
  asset   $ASSET_ID
  manager $MANAGER_ID

Next, set a yield rate on the vault, then open an epoch on the manager.
Deposits must close before maturity, so open the epoch and deposit promptly.

  stellar contract invoke --id strata-mock-vault \\
    --source-account $IDENTITY --network $NETWORK -- set_yield_rate --rate_bps 500

  stellar contract invoke --id strata-epoch-manager \\
    --source-account $IDENTITY --network $NETWORK -- create_epoch \\
    --term_seconds 31536000 --rate_bps 500 --max_senior_ratio_bps 10000

Note the gate: senior deposits are refused until junior capital exists, so
deposit the junior tranche first.
EOF
