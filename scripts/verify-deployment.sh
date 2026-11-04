#!/usr/bin/env bash

# Strata deployment verifier — TESTNET ONLY. UNAUDITED.
#
# Checks that the contracts recorded in `deployments/testnet.json` are still
# live on Stellar testnet, that their on-chain Wasm hashes match the recorded
# `wasm_sha256` values (and optionally a fresh local `stellar contract build`),
# and that each contract answers a read-only view call.
#
# Usage:
#   ./scripts/verify-deployment.sh [network] [--rebuild]
#   ./scripts/verify-deployment.sh --self-test
#   ./scripts/verify-deployment.sh --help
#
# Defaults to `testnet`. Refuses any network other than `testnet`: Strata is
# unaudited and has no mainnet code path.
#
# Exit codes and failure modes:
#   0  All recorded contracts exist on chain, hashes match, and view calls pass
#   1  Usage, guard, build, or read-only invocation failure
#   2  Contract not found on the ledger (for example after a testnet reset or archival)
#   3  Wasm sha256 hash mismatch (on-chain vs deployments/testnet.json or local build)

set -euo pipefail

# Only `testnet` is accepted here. `verify-deployment.sh` checks the committed
# `deployments/testnet.json` against Stellar testnet; reusing the guard pattern
# from `scripts/deploy-testnet.sh` ensures asking for `mainnet` fails on the
# network guard before anything else is inspected.
ALLOWED_NETWORKS=("testnet")

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

WASM_DIR="target/wasm32v1-none/release"
RECORD_DIR="deployments"

say() { printf '%s\n' "$*"; }
step() { say ""; say "==> $*"; }
die() {
    say "error: $*" >&2
    exit 1
}

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
    sed -n '3,21p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

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

# Pull a top-level string field out of deployments/<network>.json.
record_top() {
    local file="$1" key="$2"
    grep -oE "\"$key\": *\"[^\"]+\"" "$file" | head -n 1 |
        sed 's/.*"://; s/^[[:space:]]*"//; s/"[[:space:]]*$//' || true
}

# Pull a field out of the record's `contracts.<name>` section, matching the
# extraction pattern in `scripts/extend-ttl-testnet.sh`.
record_contract() {
    local file="$1" contract="$2" key="$3"
    grep -A 8 "\"$contract\":" "$file" | grep -oE "\"$key\": *\"[^\"]+\"" | head -n 1 |
        sed 's/.*"://; s/^[[:space:]]*"//; s/"[[:space:]]*$//' || true
}

# Discovered by glob rather than hardcoded, matching `scripts/deploy-testnet.sh`.
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

# Classifies the result of `stellar contract info hash` into:
#   0  exact match with expected_hash
#   2  contract not found on ledger (testnet reset, expired instance, or RPC 404)
#   3  hash mismatch between on-chain Wasm and recorded wasm_sha256
classify_hash_check() {
    local label="$1"
    local contract_id="$2"
    local expected_hash="$3"
    local cli_status="$4"
    local cli_output="$5"

    local actual_hash
    actual_hash=$(printf '%s\n' "$cli_output" | grep -oE '\b[0-9a-f]{64}\b' | tail -n 1 || true)

    if [ "$cli_status" -ne 0 ] || [ -z "$actual_hash" ]; then
        say "error: contract not found: $label ($contract_id) is missing on the ledger (testnet may have reset or the contract instance expired)." >&2
        if [ -n "$cli_output" ]; then
            say "  cli output: $cli_output" >&2
        fi
        return 2
    fi

    if [ "$actual_hash" != "$expected_hash" ]; then
        say "error: hash mismatch for $label ($contract_id): recorded wasm_sha256 is $expected_hash, but on-chain wasm hash is $actual_hash." >&2
        return 3
    fi

    say "  ok   $label ($contract_id) on-chain wasm_sha256 = $actual_hash"
    return 0
}

# Compares a locally built Wasm artefact's sha256 against the recorded hash.
check_local_hash() {
    local label="$1"
    local wasm_path="$2"
    local expected_hash="$3"

    local local_hash
    local_hash=$(sha256_of "$wasm_path")
    if [ "$local_hash" != "$expected_hash" ]; then
        say "error: hash mismatch for local build of $label ($wasm_path): recorded wasm_sha256 is $expected_hash, but local artefact sha256 is $local_hash." >&2
        return 3
    fi

    say "  ok   $label local build ($wasm_path) matches wasm_sha256 = $local_hash"
    return 0
}

# --- self-test ---------------------------------------------------------------
#
# Runs with no network and no credentials. Exercised in CI on every push and PR.
if [ "${1:-}" = "--self-test" ]; then
    for good in "${ALLOWED_NETWORKS[@]}"; do
        assert_network_is_allowed "$good" || {
            echo "FAIL: rejected $good"
            exit 1
        }
        echo "ok: $good allowed"
    done
    for bad in mainnet public "Main Net" "" local futurenet futurenetX; do
        if assert_network_is_allowed "$bad"; then
            echo "FAIL: accepted '$bad'"
            exit 1
        fi
        echo "ok: '$bad' refused"
    done

    # Check sha256_of helper.
    tmp_hash_probe="$(mktemp)"
    echo "probe" >"$tmp_hash_probe"
    probe_hash="$(sha256_of "$tmp_hash_probe")"
    if [ ${#probe_hash} -ne 64 ]; then
        rm -f "$tmp_hash_probe"
        echo "FAIL: sha256_of returned '$probe_hash'"
        exit 1
    fi
    echo "ok: sha256_of works"

    # Check deployments/testnet.json parsing.
    record_file="$RECORD_DIR/testnet.json"
    if [ ! -f "$record_file" ]; then
        rm -f "$tmp_hash_probe"
        echo "FAIL: $record_file not found"
        exit 1
    fi
    rec_net="$(record_top "$record_file" network)"
    rec_admin="$(record_top "$record_file" admin_public_address)"
    v_id="$(record_contract "$record_file" mock_vault contract_id)"
    v_hash="$(record_contract "$record_file" mock_vault wasm_sha256)"
    m_id="$(record_contract "$record_file" epoch_manager contract_id)"
    m_hash="$(record_contract "$record_file" epoch_manager wasm_sha256)"

    [ "$rec_net" = "testnet" ] || { rm -f "$tmp_hash_probe"; echo "FAIL: record network is '$rec_net'"; exit 1; }
    printf '%s' "$rec_admin" | grep -qE '^G[0-9A-Z]{55}$' || { rm -f "$tmp_hash_probe"; echo "FAIL: invalid admin_public_address '$rec_admin'"; exit 1; }
    printf '%s' "$v_id" | grep -qE '^C[0-9A-Z]{55}$' || { rm -f "$tmp_hash_probe"; echo "FAIL: invalid mock_vault contract_id '$v_id'"; exit 1; }
    printf '%s' "$m_id" | grep -qE '^C[0-9A-Z]{55}$' || { rm -f "$tmp_hash_probe"; echo "FAIL: invalid epoch_manager contract_id '$m_id'"; exit 1; }
    printf '%s' "$v_hash" | grep -qE '^[0-9a-f]{64}$' || { rm -f "$tmp_hash_probe"; echo "FAIL: invalid mock_vault wasm_sha256 '$v_hash'"; exit 1; }
    printf '%s' "$m_hash" | grep -qE '^[0-9a-f]{64}$' || { rm -f "$tmp_hash_probe"; echo "FAIL: invalid epoch_manager wasm_sha256 '$m_hash'"; exit 1; }
    echo "ok: $record_file parsed cleanly"

    # Check distinct failure classifications:
    # 1. Matching hash succeeds.
    classify_hash_check "mock_vault" "$v_id" "$v_hash" 0 "$v_hash" >/dev/null || {
        rm -f "$tmp_hash_probe"
        echo "FAIL: matching hash was rejected"
        exit 1
    }
    echo "ok: matching on-chain hash accepted"

    # 2. Missing contract exits non-zero with "contract not found".
    set +e
    missing_msg="$(classify_hash_check "mock_vault" "$v_id" "$v_hash" 1 "error: Contract not found" 2>&1)"
    missing_rc=$?
    set -e
    if [ "$missing_rc" -eq 0 ] || ! printf '%s' "$missing_msg" | grep -q "contract not found"; then
        rm -f "$tmp_hash_probe"
        echo "FAIL: missing contract did not report 'contract not found' (rc=$missing_rc, msg=$missing_msg)"
        exit 1
    fi
    echo "ok: missing contract reports 'contract not found' (exit $missing_rc)"

    # 3. Wrong on-chain hash exits non-zero with "hash mismatch".
    wrong_hash="0000000000000000000000000000000000000000000000000000000000000000"
    set +e
    mismatch_msg="$(classify_hash_check "mock_vault" "$v_id" "$v_hash" 0 "$wrong_hash" 2>&1)"
    mismatch_rc=$?
    set -e
    if [ "$mismatch_rc" -eq 0 ] || ! printf '%s' "$mismatch_msg" | grep -q "hash mismatch"; then
        rm -f "$tmp_hash_probe"
        echo "FAIL: mismatched hash did not report 'hash mismatch' (rc=$mismatch_rc, msg=$mismatch_msg)"
        exit 1
    fi
    echo "ok: wrong on-chain hash reports 'hash mismatch' (exit $mismatch_rc)"

    # 4. Local artefact hash check reports "hash mismatch" on discrepancy.
    check_local_hash "probe" "$tmp_hash_probe" "$probe_hash" >/dev/null
    set +e
    local_mismatch_msg="$(check_local_hash "probe" "$tmp_hash_probe" "$wrong_hash" 2>&1)"
    local_mismatch_rc=$?
    set -e
    rm -f "$tmp_hash_probe"
    if [ "$local_mismatch_rc" -eq 0 ] || ! printf '%s' "$local_mismatch_msg" | grep -q "hash mismatch"; then
        echo "FAIL: local artefact mismatch did not report 'hash mismatch'"
        exit 1
    fi
    echo "ok: local artefact mismatch reports 'hash mismatch' (exit $local_mismatch_rc)"

    echo "verify-deployment self-test passed"
    exit 0
fi

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    usage
    exit 0
fi

NETWORK="testnet"
REBUILD=0

for arg in "$@"; do
    case "$arg" in
        --rebuild)
            REBUILD=1
            ;;
        --*)
            die "unexpected flag '$arg'. See --help."
            ;;
        *)
            NETWORK="$arg"
            ;;
    esac
done

# Network guard runs first, before checking for the Stellar CLI or reading files.
assert_network_is_allowed "$NETWORK" || die "network '$NETWORK' is not allowed.
Strata is unaudited and verify-deployment.sh only checks testnet. Allowed: ${ALLOWED_NETWORKS[*]}"

RECORD="$RECORD_DIR/$NETWORK.json"
[ -f "$RECORD" ] || die "$RECORD not found. Run scripts/deploy-testnet.sh first."

ADMIN=$(record_top "$RECORD" admin_public_address)
VAULT_ID=$(record_contract "$RECORD" mock_vault contract_id)
VAULT_HASH=$(record_contract "$RECORD" mock_vault wasm_sha256)
MANAGER_ID=$(record_contract "$RECORD" epoch_manager contract_id)
MANAGER_HASH=$(record_contract "$RECORD" epoch_manager wasm_sha256)

[ -n "$ADMIN" ] && [ -n "$VAULT_ID" ] && [ -n "$VAULT_HASH" ] && [ -n "$MANAGER_ID" ] && [ -n "$MANAGER_HASH" ] ||
    die "could not read contract IDs, wasm hashes, and admin_public_address from $RECORD"

command -v stellar >/dev/null 2>&1 ||
    die "stellar CLI not found. Install it, then re-run. See docs/deployment.md."

say "Verifying $RECORD against Stellar $NETWORK. UNAUDITED."
say "  vault   $VAULT_ID"
say "  manager $MANAGER_ID"
say "  admin   $ADMIN"

# --- optional local rebuild check --------------------------------------------

if [ "$REBUILD" -eq 1 ]; then
    step "building contracts locally (--rebuild)"
    stellar contract build

    VAULT_WASM=$(find_wasm mock_vault) ||
        die "no wasm matching *mock_vault* in $WASM_DIR after 'stellar contract build'"
    MANAGER_WASM=$(find_wasm epoch_manager) ||
        die "no wasm matching *epoch_manager* in $WASM_DIR after 'stellar contract build'"

    check_local_hash "mock_vault" "$VAULT_WASM" "$VAULT_HASH" || exit $?
    check_local_hash "epoch_manager" "$MANAGER_WASM" "$MANAGER_HASH" || exit $?
fi

# --- on-chain wasm hash verification -----------------------------------------

step "checking on-chain wasm hashes against $RECORD"

for pair in "mock_vault:$VAULT_ID:$VAULT_HASH" "epoch_manager:$MANAGER_ID:$MANAGER_HASH"; do
    label="${pair%%:*}"
    rest="${pair#*:}"
    cid="${rest%%:*}"
    expected="${rest#*:}"

    set +e
    info_out=$(stellar contract info hash --id "$cid" --network "$NETWORK" 2>&1)
    info_rc=$?
    set -e

    classify_hash_check "$label" "$cid" "$expected" "$info_rc" "$info_out" || exit $?
done

# --- read-only contract function verification --------------------------------

step "calling one read-only function per contract"

# Use the recorded admin public key as `--source-account` with `--send=no` so
# verifying a deployment requires no secret key in the local identity store.
call_readonly() {
    local label="$1" cid="$2" fn="$3"
    local out rc
    set +e
    out=$(stellar contract invoke \
        --id "$cid" \
        --source-account "$ADMIN" \
        --network "$NETWORK" \
        --no-cache \
        --send=no \
        -- \
        "$fn" 2>&1)
    rc=$?
    set -e

    if [ "$rc" -ne 0 ]; then
        if printf '%s\n' "$out" | grep -qiE 'not found|missing|does not exist'; then
            say "error: contract not found: $label ($cid) failed read-only call '$fn' because the contract entry is missing on $NETWORK." >&2
            say "  cli output: $out" >&2
            exit 2
        fi
        say "error: read-only call '$fn' on $label ($cid) failed (exit $rc):" >&2
        say "  $out" >&2
        exit 1
    fi
    say "  ok   $label.$fn() -> $(printf '%s' "$out" | tail -n 1)"
}

call_readonly "mock_vault" "$VAULT_ID" "total_assets"
call_readonly "epoch_manager" "$MANAGER_ID" "current_epoch"

step "done"
say "All contracts in $RECORD are live on $NETWORK, on-chain wasm hashes match, and read-only calls succeeded."
