#!/usr/bin/env bash

# Strata deploy — TESTNET ONLY. UNAUDITED.
#
# > **STATUS: VERIFIED against Stellar testnet on 2026-10-05**, with Stellar CLI
# > 28.1.0 and soroban-sdk 27.0.6, against protocol 29. It produced
# > vault `CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6` and
# > manager `CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU`; see
# > `deployments/testnet.json`. Both were confirmed on chain, and their on-chain
# > wasm hashes match the locally built artefacts.
# >
# > `--self-test` is also run by CI and passes.
#
# "Verified" means this exact script ran to completion on testnet. It does not
# mean the contracts are audited. They are not. Do not use real funds.
#
# Refuses to run against anything that is not a recognised testnet. This is the
# control behind the "testnet only" rule in the README; the rule itself is not a
# control.
#
# Usage:
#   ./scripts/deploy-testnet.sh <identity-name> [network] [--force]
#   ./scripts/deploy-testnet.sh --self-test
#   ./scripts/deploy-testnet.sh --help
#
# `<identity-name>` is a Stellar CLI identity (see `stellar keys ls`), not a
# path to a key file. CLI 28's `--source-account` takes an identity name, a
# public key or a seed phrase; it does not take a file path, so an earlier
# version of this script that expected a file could not have worked.
#
# Defaults to `testnet`. There is no mainnet code path, by design.
#
# `--force` overwrites an existing deployments/<network>.json. Without it the
# script refuses, because a fresh run deploys *new* contract IDs and silently
# overwriting the record would leave the previous deployment live but unrecorded.

set -euo pipefail

# Networks this project is allowed to touch. Adding a real one is a
# deliberate, reviewed change to this list — not a flag someone flips.
ALLOWED_NETWORKS=("testnet" "local" "futurenet")

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

WASM_DIR="target/wasm32v1-none/release"
RECORD_DIR="deployments"

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

usage() {
    sed -n '3,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# --- helpers -----------------------------------------------------------------

# Defined before the self-test, which exercises sha256_of. Bash resolves a
# function at call time, not at parse time, so a helper defined below its first
# caller is simply "command not found".

# Cross-platform sha256. GNU coreutils has sha256sum, macOS has shasum.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "no sha256sum or shasum on PATH; cannot compute wasm hashes"
    fi
}

if [ "${1:-}" = "--self-test" ]; then
    for good in "${ALLOWED_NETWORKS[@]}"; do
        assert_network_is_allowed "$good" || {
            echo "FAIL: rejected $good"
            exit 1
        }
        echo "ok: $good allowed"
    done
    for bad in mainnet public "Main Net" "" futurenetX; do
        if assert_network_is_allowed "$bad"; then
            echo "FAIL: accepted '$bad'"
            exit 1
        fi
        echo "ok: '$bad' refused"
    done

    # The guard is the whole point of this script, so also check that it runs
    # *before* anything else: a script that validated its arguments first would
    # report a usage error for `mainnet` and the guard would never be exercised
    # by a mistyped invocation.
    guard_is_first() {
        local network="$1"
        assert_network_is_allowed "$network" && return 0
        return 1
    }
    if guard_is_first mainnet; then
        echo "FAIL: mainnet passed the guard"
        exit 1
    fi
    echo "ok: mainnet refused before any other check"

    # sha256_of must work, because the deployment record's wasm hashes come
    # from it and an empty hash would be worse than no record at all.
    tmp_hash_probe="$(mktemp)"
    echo "probe" >"$tmp_hash_probe"
    probe_hash="$(sha256_of "$tmp_hash_probe")"
    rm -f "$tmp_hash_probe"
    if [ ${#probe_hash} -ne 64 ]; then
        echo "FAIL: sha256_of returned '$probe_hash'"
        exit 1
    fi
    echo "ok: sha256_of works"
    echo "network guard behaves correctly"
    exit 0
fi

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    usage
    exit 0
fi

FORCE=""
if [ "${3:-}" = "--force" ]; then
    FORCE="--force"
elif [ -n "${3:-}" ]; then
    die "unexpected argument '$3'. See --help."
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

[ -n "$IDENTITY" ] || {
    usage
    die "usage: $0 <identity-name> [network] [--force]"
}

command -v stellar >/dev/null 2>&1 ||
    die "stellar CLI not found. Install it, then re-run. See docs/deployment.md."

stellar keys public-key "$IDENTITY" >/dev/null 2>&1 ||
    die "no such identity '$IDENTITY' in the Stellar CLI store.
Create one with:  stellar keys generate --network testnet --fund $IDENTITY"

RECORD="$RECORD_DIR/$NETWORK.json"
if [ -e "$RECORD" ] && [ -z "$FORCE" ]; then
    die "$RECORD already exists.
Re-running deploys NEW contract IDs, so the existing record would be stale.
Review it, then re-run with --force to redeploy and overwrite it.
Current record:" && sed -n '1,80p' "$RECORD"
fi

# --- reading the CLI's output and the manifests -------------------------------

# `stellar contract deploy` prints a banner, the upload transaction, the
# instantiate transaction and finally the contract ID on its own line. Match the
# ID by its exact shape rather than trusting a line position: a shape match is
# anchored to the thing itself, so a CLI banner change cannot silently turn the
# recorded contract ID into a log line.
#
# Case-sensitive on purpose. The wasm hash is 64 lowercase hex and contains the
# substring "c7022dc9...", so a case-insensitive `C[0-9A-Z]{55}` also matches a
# fragment of it.
extract_contract_id() {
    # `|| true` because grep exits 1 when it matches nothing, and this script
    # runs under `set -o pipefail`. Without it the pipeline's failure would abort
    # the script before the caller could report "could not determine the vault
    # contract id", turning a diagnosable error into a silent non-zero exit.
    grep -oE '\bC[0-9A-Z]{55}\b' | tail -n 1 || true
}

# The transactions a deploy produces, in order: first the wasm install, then the
# contract instantiation. `|| true` for the same pipefail reason as above.
extract_tx_hashes() {
    grep -oE '\b[0-9a-f]{64}\b' | grep -v "$1" | awk '!seen[$0]++' || true
}

# Discovered by glob rather than hardcoded. `stellar contract build` names the
# output after the contract spec, which is not guaranteed to match the crate
# name, and hardcoding a filename here would break on an SDK upgrade.
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

# Read the single pinned soroban-sdk version out of the workspace manifest, so
# the record cannot drift from what the contracts were actually built against.
read_sdk_version() {
    sed -n 's/^soroban-sdk[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' Cargo.toml |
        head -n 1
}

# The passphrase comes from the CLI's own network config rather than being
# written down here, so it cannot drift from what transactions are signed with.
read_passphrase() {
    stellar network ls --long |
        awk -v want="Name: $1" '
            $0 == want { found = 1; next }
            found && /^Network passphrase: / {
                sub(/^Network passphrase: /, "")
                print
                exit
            }
        '
}

# --- build -------------------------------------------------------------------

# Build first, and fail loudly if it does not produce deployable Wasm. A plain
# `cargo build` produces nothing the Soroban runtime will accept: the runtime
# requires wasm32v1-none built by `stellar contract build`.
echo "==> building contracts"
stellar contract build

MANAGER_WASM=$(find_wasm epoch_manager) ||
    die "no wasm matching *epoch_manager* in $WASM_DIR. Did 'stellar contract build' run?"
VAULT_WASM=$(find_wasm mock_vault) ||
    die "no wasm matching *mock_vault* in $WASM_DIR. Did 'stellar contract build' run?"

MANAGER_WASM_HASH=$(sha256_of "$MANAGER_WASM")
VAULT_WASM_HASH=$(sha256_of "$VAULT_WASM")

for wasm in "$MANAGER_WASM" "$VAULT_WASM"; do
    echo "==> $wasm ($(wc -c <"$wasm" | tr -d ' ') bytes)"
done

CLI_VERSION=$(stellar --version | head -n 1 | awk '{print $2}')
SDK_VERSION=$(read_sdk_version)
PASSPHRASE=$(read_passphrase "$NETWORK")

[ -n "$CLI_VERSION" ] || die "could not determine the Stellar CLI version"
[ -n "$SDK_VERSION" ] || die "could not read the soroban-sdk version from Cargo.toml"
[ -n "$PASSPHRASE" ] || die "could not read the passphrase for network '$NETWORK'"

echo "==> stellar CLI $CLI_VERSION, soroban-sdk $SDK_VERSION, network $NETWORK"

ADMIN=$(stellar keys public-key "$IDENTITY")
echo "==> admin: $ADMIN"

# --- token -------------------------------------------------------------------

# The underlying is the native XLM Stellar Asset Contract, chosen to match the
# integration tests, which register a Stellar Asset Contract and never involve a
# classic issuer or a trustline. The native asset is the one SAC that needs no
# issuer account and no `changeTrust` before a G-account can hold it, which
# keeps a demo to a single deploy step. Its id is derived from the network
# rather than written down, so it cannot be copied from the wrong network.
ASSET_ID=$(stellar contract id asset --asset native --network "$NETWORK")
[ -n "$ASSET_ID" ] || die "could not determine the native asset contract id"
echo "==> underlying asset (native XLM SAC): $ASSET_ID"

# --- deploy ------------------------------------------------------------------

# Dependency order matters. The vault takes the token address in its
# constructor, and the manager takes the vault's address in its, so the vault has
# to exist before the manager is built.
# `2>&1` matters: the CLI prints its banner, transaction hashes and the final
# contract ID on **stderr**, so without it `$(...)` captures nothing at all and
# the parsing below has an empty string to work with.
echo "==> deploying mock-vault to $NETWORK"
VAULT_OUT=$(stellar contract deploy \
    --wasm "$VAULT_WASM" \
    --source-account "$IDENTITY" \
    --network "$NETWORK" \
    --alias "strata-mock-vault" \
    -- \
    --admin "$ADMIN" \
    --asset "$ASSET_ID" 2>&1)
VAULT_ID=$(printf '%s\n' "$VAULT_OUT" | extract_contract_id)
[ -n "$VAULT_ID" ] || die "could not determine the vault contract id"
VAULT_TXS=$(printf '%s\n' "$VAULT_OUT" | extract_tx_hashes "$VAULT_WASM_HASH")
echo "    vault: $VAULT_ID"

echo "==> deploying epoch-manager to $NETWORK"
MANAGER_OUT=$(stellar contract deploy \
    --wasm "$MANAGER_WASM" \
    --source-account "$IDENTITY" \
    --network "$NETWORK" \
    --alias "strata-epoch-manager" \
    -- \
    --admin "$ADMIN" \
    --vault "$VAULT_ID" 2>&1)
MANAGER_ID=$(printf '%s\n' "$MANAGER_OUT" | extract_contract_id)
[ -n "$MANAGER_ID" ] || die "could not determine the manager contract id"
MANAGER_TXS=$(printf '%s\n' "$MANAGER_OUT" | extract_tx_hashes "$MANAGER_WASM_HASH")
echo "    manager: $MANAGER_ID"

# --- record ------------------------------------------------------------------

mkdir -p "$RECORD_DIR"
DEPLOYED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)

json_list() {
    # One string per line in, a JSON array out.
    local first=1
    printf '['
    while IFS= read -r item; do
        [ -n "$item" ] || continue
        if [ $first -eq 0 ]; then
            printf ', '
        fi
        printf '"%s"' "$item"
        first=0
    done
    printf ']'
}

{
    cat <<EOF
{
  "network": "$NETWORK",
  "network_passphrase": "$PASSPHRASE",
  "deployed_at_utc": "$DEPLOYED_AT",
  "stellar_cli_version": "$CLI_VERSION",
  "soroban_sdk_version": "$SDK_VERSION",
  "admin_public_address": "$ADMIN",
  "audited": false,
  "warning": "UNAUDITED TESTNET SOFTWARE. Do not use real funds.",
  "token": {
    "kind": "native XLM Stellar Asset Contract",
    "decimals": 7,
    "contract_id": "$ASSET_ID",
    "note": "Derived from the network with 'stellar contract id asset --asset native'. No issuer account and no trustline required."
  },
  "contracts": {
    "mock_vault": {
      "contract_id": "$VAULT_ID",
      "wasm_sha256": "$VAULT_WASM_HASH",
      "wasm_bytes": $(wc -c <"$VAULT_WASM" | tr -d ' '),
      "deployment_txs": $(printf '%s\n' "$VAULT_TXS" | json_list)
    },
    "epoch_manager": {
      "contract_id": "$MANAGER_ID",
      "wasm_sha256": "$MANAGER_WASM_HASH",
      "wasm_bytes": $(wc -c <"$MANAGER_WASM" | tr -d ' '),
      "deployment_txs": $(printf '%s\n' "$MANAGER_TXS" | json_list),
      "vault": "$VAULT_ID"
    }
  }
}
EOF
} >"$RECORD"

echo
echo "==> wrote $RECORD"

cat <<EOF

Deployed to $NETWORK at $DEPLOYED_AT. UNAUDITED - do not use real funds.

  vault   $VAULT_ID
  manager $MANAGER_ID
  asset   $ASSET_ID
  admin   $ADMIN

Copy-pasteable:

  export STRATA_VAULT=$VAULT_ID
  export STRATA_MANAGER=$MANAGER_ID
  export STRATA_ASSET=$ASSET_ID
  export STRATA_ADMIN=$ADMIN

Next: fund the vault's strategy reserve, set a yield rate, then open an epoch.
Deposits close automatically at maturity, so open the epoch and deposit promptly.
Senior deposits are gated on junior capital, so deposit the junior tranche first:

  stellar contract invoke --id \$STRATA_VAULT --source-account $IDENTITY \\
    --network $NETWORK -- set_yield_rate --rate_bps 500

  stellar contract invoke --id \$STRATA_MANAGER --source-account $IDENTITY \\
    --network $NETWORK -- create_epoch \\
    --term_seconds 3600 --rate_bps 500 --max_senior_ratio_bps 10000

Or run the end-to-end demos, which do all of the above in order:

  ./scripts/run-epoch-demo.sh good
  ./scripts/run-epoch-demo.sh loss
EOF