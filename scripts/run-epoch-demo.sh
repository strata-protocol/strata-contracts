#!/usr/bin/env bash

# Strata epoch demo — TESTNET ONLY. UNAUDITED.
#
# > **STATUS: VERIFIED on Stellar testnet on 2026-10-05**, both scenarios, with
# > Stellar CLI 28.1.0 and soroban-sdk 27.0.6, against protocol 29.
# >
# > `good`, term 300s, senior target 10_000bps, vault yield +100_000bps:
# > `V = 2_000_204_528`, `senior_due = senior_payout = 1_000_009_512`,
# > `junior_payout = 1_000_195_016`. The senior took 9_512 of the vault's
# > 204_528 gain, exactly its target, and the junior took the remaining 195_016:
# > the cap binding, which is the whole point of the senior tranche.
# >
# > `loss`, same terms, vault yield -100_000bps:
# > `V = 1_999_836_692`, `senior_due = senior_payout = 1_000_009_512`,
# > `junior_payout = 999_827_180`. The senior was made whole and still gained its
# > 9_512 target; the junior lost 172_820 of its 1_000_000_000. Paid first, junior
# > absorbs first.
# >
# > In both runs the two payouts summed to `V` exactly, and each tranche paid out
# > exactly its own payout across the claims.
#
# "Verified" means this script ran to completion on testnet with every assertion
# passing. It does not mean the contracts are audited. They are not. Do not use
# real funds.
#
# Runs one complete epoch on Stellar testnet and checks the settlement against
# `docs/waterfall-spec.md`, recomputed here from the spec's own formulas rather
# than read back out of the contract.
#
# Usage:
#   ./scripts/run-epoch-demo.sh <good|loss> [network] [--term-seconds N]
#
# The contract IDs come from deployments/<network>.json, the file the deploy
# script wrote, so this cannot drift onto a different deployment.
#
# Requires identities `strata-deployer` (contract admin), `strata-senior` and
# `strata-junior` in the Stellar CLI store. Refuses any network but testnet.
#
# What each scenario asserts, all derived from the spec:
#   * `senior_due == S + (S*r*t) / (10_000 * 31_536_000)`
#   * invariant 1  senior_payout + junior_payout == V
#   * invariant 2  senior_payout <= senior_due
#   * invariant 3  junior_payout > 0 implies senior_payout == senior_due
#   * the senior is capped at its target and the junior takes the excess
#   * one depositor per tranche receives exactly that tranche's payout, via the
#     last-claim remainder path in spec section 8
#
# `good` runs the maximum positive vault yield and expects the senior capped at
# its target with the junior taking the excess. `loss` runs the maximum negative
# vault yield the mock allows and expects the junior to absorb the whole loss
# while the senior is made whole, then uses the manager's `project` view to show
# the cushion boundary that a short demo cannot reach with a real loss.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Scenario parameters. The senior target rate and the vault rate are both at
# their documented maxima (epoch-manager allows rate_bps <= 10_000, mock-vault
# allows |rate_bps| <= 100_000) so the arithmetic is visible at this short term.
TERM_SECONDS=300
SENIOR_RATE_BPS=10000 # 100%/yr, the manager's documented ceiling
SENIOR_RATIO_BPS=10000 # 1.0x, so senior principal may equal junior principal
GOOD_VAULT_RATE_BPS=100000 # +1000%/yr, the mock's documented ceiling
LOSS_VAULT_RATE_BPS=-100000 # -1000%/yr, the mock's documented floor

# 100 XLM a side. Equal, so the senior deposit lands exactly on the gate's cap
# and `senior_room` goes to zero: the buffer is exactly as large as the senior.
DEPOSIT_AMOUNT=1000000000

# The mock's yield is unbacked accounting, so the vault must hold real tokens to
# pay a redemption that includes a gain. This is the integration tests' "strategy
# reserve"; see docs/architecture.md and the mock vault's crate docs.
RESERVE_TARGET=500000000 # 50 XLM

READ_ATTEMPTS=5

ADMIN_IDENTITY="strata-deployer"
SENIOR_IDENTITY="strata-senior"
JUNIOR_IDENTITY="strata-junior"

SCENARIO="${1:-}"
NETWORK="testnet"
if [ "${2:-}" != "" ] && [ "${2:-}" != "--term-seconds" ]; then
    NETWORK="$2"
    shift 2
else
    shift || true
fi
if [ "${1:-}" = "--term-seconds" ]; then
    TERM_SECONDS="$2"
    shift 2
fi

TX_LOG=()
TX_OUT=""
FAILURES=0

say() { printf '%s\n' "$*"; }
step() { say ""; say "==> $*"; }
fail() {
    say "FAIL: $*"
    FAILURES=$((FAILURES + 1))
}
die() {
    say "error: $*" >&2
    exit 1
}

expect_eq() {
    local what="$1" want="$2" got="$3"
    if [ "$want" = "$got" ]; then
        say "  ok   $what = $got"
    else
        fail "$what: expected $want, got $got"
    fi
}

expect_true() {
    local what="$1" cond="$2"
    if [ "$cond" = "true" ]; then
        say "  ok   $what"
    else
        fail "$what"
    fi
}

# --- CLI plumbing ------------------------------------------------------------

# `stellar contract invoke` prints its banner on stderr, so every capture merges
# the two streams. Without that the captures below would be empty.
#
# `--no-cache` is not optional. The CLI caches simulations, and a cached one can
# carry a footprint that no longer covers the state the transaction actually
# meets. Observed on testnet: a senior `deposit` passed simulation and then failed
# on submission with
#     {"storage":"exceeded_limit"}
#     trying to access contract data key outside of the footprint ... [RateBps]
# because the simulation's footprint omitted the mock vault's rate entry. The
# identical transaction submitted with `--no-cache` succeeded first time. A stale
# footprint is a real risk in a script that runs a long sequence of interacting
# transactions against one manager and one vault, so the cache is off throughout.
invoke() {
    local source="$1" id="$2"
    shift 2
    stellar contract invoke \
        --id "$id" \
        --source-account "$source" \
        --network "$NETWORK" \
        --no-cache \
        "$@" 2>&1
}

# One read attempt, never fatal. `|| true` because a simulation of a call that is
# *meant* to fail — the gate check below deliberately deposits past the cap —
# makes the CLI exit non-zero, and under `set -e` that would abort the script
# before it could report anything.
read_once() {
    local id="$1" fn="$2"
    shift 2
    invoke "$ADMIN_IDENTITY" "$id" --send=no -- "$fn" "$@" 2>&1 || true
}

# --- parsing the CLI's output ------------------------------------------------
#
# The CLI has no JSON output mode, and it prints a struct as a single-line JSON
# object with i128 fields quoted ("value_redeemed":"123") and u64 fields bare
# ("term_seconds":300).

# Pull one integer out of a struct object. The quoted field name anchors the
# match, so `senior_paid` can never be read out of `senior_payout`, and no
# trailing `}` is required, because the fields come back in alphabetical order
# and anchoring on the brace would only match whichever field happened to be last.
json_field() {
    printf '%s' "$1" |
        grep -oE "\"$2\":\"?[0-9]+\"?" |
        grep -oE '[0-9]+' | head -n 1 || true
}

# A string field, e.g. `"status":"Closed"`. Separate from json_field because that
# one only matches digits and a string field read with a numeric pattern silently
# yields nothing. Both quotes around the value have to come off separately: the
# `.*":` strip is greedy and runs through to the colon before the value.
json_str() {
    printf '%s' "$1" | grep -oE "\"$2\":\"[^\"]*\"" | head -n 1 |
        sed 's/.*"://; s/^"//; s/"$//' || true
}

# A bare i128 return, printed as a quoted number.
scalar() {
    printf '%s' "$1" | tr -d '"' | grep -oE '^-?[0-9]+' | head -n 1 || true
}

# --- retrying reads ----------------------------------------------------------
#
# Testnet's public RPC drops connections intermittently. An earlier version of
# this script retried on the *wording* of the CLI's error and still let a dropped
# read through to be reported as a contract answer, which is the worst possible
# failure here: a network blip would be recorded as a wrong settlement. So the
# retry condition is the absence of a parseable answer, not the presence of a
# particular string. That is stable across CLI versions and transport errors.
#
# `read_obj` returns the whole object so a caller can take several fields from
# one round trip. Reading `current_epoch` once per field instead of once per
# field-per-call is both faster and far less exposed to a blip.

read_retrying() {
    local what="$1" id="$2" fn="$3"
    shift 3
    local attempt out
    for attempt in $(seq 1 "$READ_ATTEMPTS"); do
        out=$(read_once "$id" "$fn" "$@")
        if [ -n "$out" ] && printf '%s' "$out" | grep -q '{'; then
            printf '%s' "$out"
            return 0
        fi
        say "  .. no object back from $what (attempt $attempt/$READ_ATTEMPTS), retrying"
        sleep 4
    done
    return 1
}

read_scalar() {
    local what="$1" id="$2" fn="$3"
    shift 3
    local attempt out val
    for attempt in $(seq 1 "$READ_ATTEMPTS"); do
        out=$(read_once "$id" "$fn" "$@")
        val=$(scalar "$out")
        if [ -n "$val" ]; then
            printf '%s' "$val"
            return 0
        fi
        say "  .. no number back from $what (attempt $attempt/$READ_ATTEMPTS), retrying"
        sleep 4
    done
    return 1
}

# Submit a transaction, and refuse to go on unless one was actually signed.
#
# Without this a failed transaction under `set -e` aborts the script with no
# indication of which step failed, and a submit that produced no hash could mean
# either "rejected" or "submitted but the reply was lost", which must not be
# retried blindly.
send_tx() {
    local label="$1" source="$2" id="$3"
    shift 3
    local out hash
    if ! out=$(invoke "$source" "$id" "$@" 2>&1); then
        printf '%s\n' "$out" >&2
        die "'$label' failed. Nothing after it was run, so the manager is left
exactly as it was. Fix the cause and re-run."
    fi
    hash=$(printf '%s' "$out" | grep -oE 'Signing transaction: [0-9a-f]{64}' |
        grep -oE '[0-9a-f]{64}' | tail -n 1 || true)
    if [ -z "$hash" ]; then
        printf '%s\n' "$out" >&2
        die "'$label' produced no transaction hash. It may or may not have been
submitted. Check the explorer before retrying it."
    fi
    TX_LOG+=("$label $hash")
    # Exposed as a global rather than echoed, because a caller's own log line
    # would otherwise land in the middle of the returned value and every parse of
    # it would be wrong.
    TX_OUT="$out"
    say "  $label -> $hash"
}

# --- the spec, recomputed here -----------------------------------------------
#
# docs/waterfall-spec.md sections 3, 4 and 5:
#   SECONDS_PER_YEAR = 31_536_000, BPS_DENOMINATOR = 10_000
#   senior_due    = S + (S * r * t) / (BPS_DENOMINATOR * SECONDS_PER_YEAR)
#   senior_payout = min(V, senior_due)
#   junior_payout = V - senior_payout
#
# Bash arithmetic is 64-bit signed. The widest intermediate here is
# S * r * t = 1e9 * 1e4 * 300 = 3e15, far inside 9.22e18, so bash gets this
# exact. It is NOT safe for arbitrary S: the contract's real domain is
# S <= 1e26 and this deliberately only handles demo-sized amounts.

spec_senior_due() {
    echo $(( $1 + ($1 * $2 * $3) / (10000 * 31536000) ))
}

spec_settle() {
    local s="$1" j="$2" r="$3" t="$4" v="$5"
    local due sp jp
    due=$(spec_senior_due "$s" "$r" "$t")
    if [ "$v" -ge "$due" ]; then sp=$due; else sp=$v; fi
    jp=$((v - sp))
    echo "$due $sp $jp"
}

# Pin those two functions against the worked examples the spec itself prints,
# before anything is sent to the network.
#
# This is the most load-bearing check in the script. Everything after it compares
# the *contract* against this implementation, so an error here would turn every
# later assertion into a false pass or a false failure. The inputs are the spec's
# own — S = J = 1_000_000, r = 500 bps/yr, one-year term, so senior_due =
# 1_050_000 — and the expected payouts are read straight out of the table in
# section 9.
self_check_spec() {
    local s=1000000 j=1000000 r=500 t=31536000
    expect_eq "spec section 4: senior_due for the worked example" "1050000" \
        "$(spec_senior_due "$s" "$r" "$t")"

    # "V senior_payout junior_payout label"
    local v want_sp want_jp label got
    while read -r v want_sp want_jp label; do
        [ -n "$v" ] || continue
        got=$(spec_settle "$s" "$j" "$r" "$t" "$v")
        expect_eq "spec section 9 [$label] senior_payout" "$want_sp" "$(echo "$got" | cut -d' ' -f2)"
        expect_eq "spec section 9 [$label] junior_payout" "$want_jp" "$(echo "$got" | cut -d' ' -f3)"
    done <<'ROWS'
2200000 1050000 1150000 healthy
1100000 1050000 50000 buffer absorbs
1049999 1049999 0 one unit past the cushion
900000 900000 0 cushion gone
0 0 0 total wipeout
ROWS
    [ "$FAILURES" -eq 0 ] ||
        die "this script's copy of the waterfall does not reproduce the worked
examples in docs/waterfall-spec.md. Fix spec_settle before trusting any result it
produces."
}

# --- preflight ---------------------------------------------------------------

case "$SCENARIO" in
    good | loss) ;;
    "") die "usage: $0 <good|loss> [network] [--term-seconds N]" ;;
    *) die "unknown scenario '$SCENARIO'. Expected 'good' or 'loss'." ;;
esac

# Testnet only, the same control the deploy script uses: a scenario that settled
# anywhere else would move money.
[ "$NETWORK" = "testnet" ] ||
    die "network '$NETWORK' is not allowed. This demo is testnet-only; Strata is
unaudited and running an epoch anywhere else is out of the question."

RECORD="deployments/$NETWORK.json"
[ -f "$RECORD" ] || die "$RECORD not found. Run scripts/deploy-testnet.sh first."

# Read a contract id out of the record by section name. Matching the section
# rather than the position matters: the record lists the token first, so "the
# first contract id" is the token, not the vault.
record_id() {
    grep -A 6 "\"$1\":" "$RECORD" | grep -oE 'C[0-9A-Z]{55}' | head -n 1
}

ASSET_ID=$(record_id token)
VAULT_ID=$(record_id mock_vault)
MANAGER_ID=$(record_id epoch_manager)
[ -n "$VAULT_ID" ] && [ -n "$MANAGER_ID" ] && [ -n "$ASSET_ID" ] ||
    die "could not read the contract ids out of $RECORD"

command -v stellar >/dev/null 2>&1 || die "stellar CLI not found"

for identity in "$ADMIN_IDENTITY" "$SENIOR_IDENTITY" "$JUNIOR_IDENTITY"; do
    stellar keys public-key "$identity" >/dev/null 2>&1 ||
        die "no identity '$identity'. Create it with:
  stellar keys generate --network testnet --fund $identity"
done

if [ "$SCENARIO" = "good" ]; then
    VAULT_RATE_BPS=$GOOD_VAULT_RATE_BPS
else
    VAULT_RATE_BPS=$LOSS_VAULT_RATE_BPS
fi

SENIOR_ADDR=$(stellar keys public-key "$SENIOR_IDENTITY")
JUNIOR_ADDR=$(stellar keys public-key "$JUNIOR_IDENTITY")

say "Strata epoch demo — scenario '$SCENARIO' on $NETWORK. UNAUDITED."
say "  vault   $VAULT_ID"
say "  manager $MANAGER_ID"
say "  asset   $ASSET_ID"
say "  term ${TERM_SECONDS}s   senior target ${SENIOR_RATE_BPS}bps   gate ${SENIOR_RATIO_BPS}bps"
say "  vault yield ${VAULT_RATE_BPS}bps   $(($DEPOSIT_AMOUNT / 10000000)) XLM per tranche"

step "checking this script's waterfall against the worked examples in the spec"
self_check_spec
say "  the spec's own table in section 9 reproduces exactly"

# The manager allows one epoch at a time and refuses to open a new one while a
# settled epoch still has unclaimed principal. Checking here means a rerun fails
# with an explanation rather than halfway through.
if ! EPOCH=$(read_retrying "current_epoch" "$MANAGER_ID" current_epoch); then
    die "could not read the manager's current epoch after $READ_ATTEMPTS attempts.
The RPC may be down; check: stellar network health --network $NETWORK"
fi
STATUS=$(json_str "$EPOCH" status)
[ -n "$STATUS" ] || die "could not read the epoch status from: $EPOCH"
if [ "$STATUS" != "Closed" ]; then
    die "the manager is not ready for a new epoch (status '$STATUS').
Claim both tranches and close the epoch first:
  stellar contract invoke --id $MANAGER_ID --source-account $SENIOR_IDENTITY \\
    --network $NETWORK -- claim --claimant $SENIOR_IDENTITY --tranche Senior
  stellar contract invoke --id $MANAGER_ID --source-account $JUNIOR_IDENTITY \\
    --network $NETWORK -- claim --claimant $JUNIOR_IDENTITY --tranche Junior
  stellar contract invoke --id $MANAGER_ID --source-account $ADMIN_IDENTITY \\
    --network $NETWORK -- close_epoch"
fi

# --- strategy reserve --------------------------------------------------------

step "the mock vault's strategy reserve"
VAULT_BAL=$(read_scalar "the vault's token balance" "$ASSET_ID" balance --id "$VAULT_ID") ||
    die "could not read the vault's token balance"
say "  vault token balance: $VAULT_BAL"
if [ "$VAULT_BAL" -lt "$RESERVE_TARGET" ]; then
    TOPUP=$((RESERVE_TARGET - VAULT_BAL))
    say "  topping up by $TOPUP stroops, so a redemption of an accrued gain can settle"
    send_tx "reserve_topup" "$ADMIN_IDENTITY" "$ASSET_ID" -- transfer \
        --from "$ADMIN_IDENTITY" --to "$VAULT_ID" --amount "$TOPUP"
    say "  vault token balance now: $(read_scalar "the vault's balance" "$ASSET_ID" balance --id "$VAULT_ID")"
else
    say "  reserve is sufficient"
fi

# --- open the epoch ----------------------------------------------------------

step "creating the epoch: term ${TERM_SECONDS}s, senior target ${SENIOR_RATE_BPS}bps, gate ${SENIOR_RATIO_BPS}bps"
send_tx "create_epoch" "$ADMIN_IDENTITY" "$MANAGER_ID" -- create_epoch \
    --term_seconds "$TERM_SECONDS" --rate_bps "$SENIOR_RATE_BPS" \
    --max_senior_ratio_bps "$SENIOR_RATIO_BPS"

EPOCH=$(read_retrying "the new epoch" "$MANAGER_ID" current_epoch) ||
    die "could not read the epoch back after creating it"
START_TS=$(json_field "$EPOCH" start_ts)
MATURITY_TS=$(json_field "$EPOCH" maturity_ts)
say "  start_ts $START_TS  maturity_ts $MATURITY_TS"
[ -n "$START_TS" ] && [ -n "$MATURITY_TS" ] || die "could not read the epoch back: $EPOCH"
expect_eq "maturity_ts - start_ts" "$TERM_SECONDS" "$((MATURITY_TS - START_TS))"

# --- deposit junior first -----------------------------------------------------
#
# Order matters and is not cosmetic: the gate refuses a senior deposit until
# junior capital exists (spec section 7), so the junior tranche must be funded
# first or the senior deposit is rejected.

step "depositing the junior tranche (it gates the senior, spec section 7)"
send_tx "deposit_junior" "$JUNIOR_IDENTITY" "$MANAGER_ID" -- deposit \
    --from "$JUNIOR_IDENTITY" --tranche Junior --amount "$DEPOSIT_AMOUNT"

ROOM=$(read_scalar "senior_room" "$MANAGER_ID" senior_room) || die "could not read senior_room"
say "  senior_room after the junior deposit: $ROOM"
expect_eq "senior_room equals the junior deposit" "$DEPOSIT_AMOUNT" "$ROOM"

step "depositing the senior tranche, exactly on the gate's cap"
send_tx "deposit_senior" "$SENIOR_IDENTITY" "$MANAGER_ID" -- deposit \
    --from "$SENIOR_IDENTITY" --tranche Senior --amount "$DEPOSIT_AMOUNT"

ROOM=$(read_scalar "senior_room" "$MANAGER_ID" senior_room) || die "could not read senior_room"
say "  senior_room after the senior deposit: $ROOM"
expect_eq "senior_room is exhausted" "0" "$ROOM"

# One stroop of senior principal beyond the cap must be refused. That refusal is
# what makes the buffer real rather than nominal. Simulated, so it costs nothing
# and changes no state.
step "confirming the gate refuses senior principal beyond the cap"
OVER_OUT=$(read_once "$MANAGER_ID" deposit --from "$SENIOR_IDENTITY" --tranche Senior --amount 1)
if printf '%s' "$OVER_OUT" | grep -qE 'SeniorGateViolated|Contract, #10'; then
    say "  ok   a senior deposit 1 stroop over the cap is refused (SeniorGateViolated)"
else
    fail "a senior deposit 1 stroop over the cap was not refused by the gate.
The simulator said:
$OVER_OUT"
fi

# --- set the vault's yield ---------------------------------------------------

step "setting the mock vault's yield to ${VAULT_RATE_BPS}bps per year"
# A negative value needs the `--flag=value` form; `--flag value` reads as another
# option.
send_tx "set_yield_rate_${VAULT_RATE_BPS}" "$ADMIN_IDENTITY" "$VAULT_ID" \
    -- set_yield_rate --rate_bps="$VAULT_RATE_BPS"
expect_eq "vault yield_rate" "$VAULT_RATE_BPS" \
    "$(read_scalar "the vault's yield rate" "$VAULT_ID" yield_rate)"

# --- wait out the term -------------------------------------------------------

step "waiting for maturity (${TERM_SECONDS}s plus transaction latency)"
WAITED=0
while :; do
    STM=$(read_scalar "seconds_to_maturity" "$MANAGER_ID" seconds_to_maturity) ||
        die "could not read seconds_to_maturity"
    [ "$STM" -le 0 ] && break
    if [ "$WAITED" -ge $((TERM_SECONDS + 900)) ]; then
        die "still ${STM}s from maturity after ${WAITED}s of waiting"
    fi
    [ $((WAITED % 40)) -eq 0 ] && say "  ${STM}s to go..."
    sleep 10
    WAITED=$((WAITED + 10))
done
say "  mature"

# What the vault is worth now. Read before settling so the number is on the
# record next to the one settlement actually used; accrual is lazy, so the two
# differ by however much time the intervening transactions took.
V_BEFORE=$(read_scalar "the vault's total_assets" "$VAULT_ID" total_assets) ||
    die "could not read the vault's total_assets"
say "  vault total_assets before settling: $V_BEFORE"

# --- the cushion, through the manager's projection view ----------------------
#
# A real loss big enough to breach the cushion is NOT reachable in a short demo,
# and that is the contract's own bounds rather than a limitation of this script.
# The largest loss the mock can produce is
#     total_assets * 100_000bps * t / (10_000 * 31_536_000)
# which is t / 3_153_600 of the vault. The cushion is J - I, roughly J out of
# S + J = 2J, so breaching it needs t > 1_576_800s — about 18.25 days, against a
# maximum term of 31_536_000s. The bounds are correct and are not changed here.
#
# `project` is the contract's own answer for an arbitrary V, so the cushion cases
# are checked against it directly instead of being simulated.

S_TOTAL=$DEPOSIT_AMOUNT
J_TOTAL=$DEPOSIT_AMOUNT
DUE_EXPECTED=$(spec_senior_due "$S_TOTAL" "$SENIOR_RATE_BPS" "$TERM_SECONDS")
I_EXPECTED=$((DUE_EXPECTED - S_TOTAL))
CUSHION=$((J_TOTAL - I_EXPECTED))

step "checking the cushion boundary through the manager's project view"
say "  S=$S_TOTAL  J=$J_TOTAL  r=$SENIOR_RATE_BPS  t=$TERM_SECONDS"
say "  senior_due = $DUE_EXPECTED   senior interest I = $I_EXPECTED   cushion J - I = $CUSHION"

check_project() {
    local label="$1" v="$2" out want
    if ! out=$(read_retrying "project($v)" "$MANAGER_ID" project --value "$v"); then
        fail "project(V=$v) returned nothing parseable after $READ_ATTEMPTS attempts"
        return
    fi
    expect_eq "project(V=$v) senior_due  [$label]" "$DUE_EXPECTED" \
        "$(json_field "$out" senior_due)"
    want=$(spec_settle "$S_TOTAL" "$J_TOTAL" "$SENIOR_RATE_BPS" "$TERM_SECONDS" "$v")
    expect_eq "project(V=$v) senior_payout [$label]" "$(echo "$want" | cut -d' ' -f2)" \
        "$(json_field "$out" senior_payout)"
    expect_eq "project(V=$v) junior_payout [$label]" "$(echo "$want" | cut -d' ' -f3)" \
        "$(json_field "$out" junior_payout)"
}

check_project "break-even, the junior gets its principal back" "$((S_TOTAL + J_TOTAL))"
check_project "exactly at the cushion limit" "$DUE_EXPECTED"
check_project "one unit past the cushion" "$((DUE_EXPECTED - 1))"
check_project "total wipeout" "0"

# --- settle ------------------------------------------------------------------

step "settling (permissionless once mature)"
send_tx "settle" "$ADMIN_IDENTITY" "$MANAGER_ID" -- settle

# `senior_due` comes from settle's return value, which is a ProjectedSplit. It is
# deliberately not read back out of `current_epoch`: the Epoch struct has no such
# field, so looking for it there finds nothing and looks like a failed read.
SETTLED="$TX_OUT"
S_DUE=$(json_field "$SETTLED" senior_due)
SETTLED_SP=$(json_field "$SETTLED" senior_payout)
SETTLED_JP=$(json_field "$SETTLED" junior_payout)

EPOCH=$(read_retrying "the settled epoch" "$MANAGER_ID" current_epoch) ||
    die "could not read the epoch back after settling"
V=$(json_field "$EPOCH" value_redeemed)
S_PAYOUT=$(json_field "$EPOCH" senior_payout)
J_PAYOUT=$(json_field "$EPOCH" junior_payout)
S_SUM=$(json_field "$EPOCH" senior_total)
J_SUM=$(json_field "$EPOCH" junior_total)
STATUS=$(json_str "$EPOCH" status)

for pair in "senior_due:$S_DUE" "value_redeemed:$V" "senior_payout:$S_PAYOUT" \
    "junior_payout:$J_PAYOUT" "senior_total:$S_SUM" "junior_total:$J_SUM"; do
    [ -n "${pair#*:}" ] || die "could not read ${pair%%:*} from settle/current_epoch"
done
expect_eq "epoch status after settling" "Settled" "$STATUS"

# settle's own return and the epoch it wrote must agree. Two independent reads of
# the same settlement, so a mismatch cannot hide behind one bad parse.
expect_eq "settle's senior_payout matches the stored one" "$S_PAYOUT" "$SETTLED_SP"
expect_eq "settle's junior_payout matches the stored one" "$J_PAYOUT" "$SETTLED_JP"

read -r WANT_DUE WANT_SP WANT_JP <<<"$(spec_settle "$S_SUM" "$J_SUM" "$SENIOR_RATE_BPS" "$TERM_SECONDS" "$V")"

say "  V             = $V"
say "  senior_due    = $S_DUE    (spec: $WANT_DUE)"
say "  senior_payout = $S_PAYOUT   (spec: $WANT_SP)"
say "  junior_payout = $J_PAYOUT   (spec: $WANT_JP)"

say "  checking against docs/waterfall-spec.md"
expect_eq "spec senior_due" "$WANT_DUE" "$S_DUE"
expect_eq "spec senior_payout" "$WANT_SP" "$S_PAYOUT"
expect_eq "spec junior_payout" "$WANT_JP" "$J_PAYOUT"
expect_eq "invariant 1: senior + junior == V" "$V" "$((S_PAYOUT + J_PAYOUT))"
if [ "$S_PAYOUT" -le "$S_DUE" ]; then
    say "  ok   invariant 2: senior_payout <= senior_due"
else
    fail "invariant 2 broken: senior_payout $S_PAYOUT > senior_due $S_DUE"
fi
if [ "$J_PAYOUT" -gt 0 ]; then
    expect_eq "invariant 3: junior paid, so the senior was made whole" "$S_DUE" "$S_PAYOUT"
else
    say "  --   invariant 3 not applicable: the junior was paid nothing"
fi

GAIN=$((V - S_SUM - J_SUM))
if [ "$SCENARIO" = "good" ]; then
    expect_eq "the senior is capped at its target" "$S_DUE" "$S_PAYOUT"
    if [ "$J_PAYOUT" -gt "$J_SUM" ]; then
        say "  ok   the junior took the excess: $((J_PAYOUT - J_SUM)) above principal"
    else
        fail "the junior did not gain in a good epoch: $J_PAYOUT against $J_SUM"
    fi
    say "  the vault gained $GAIN. The senior took $((S_PAYOUT - S_SUM)) of it, which is"
    say "  exactly its target, and the junior took $((J_PAYOUT - J_SUM))."
else
    expect_eq "the senior is made whole" "$S_DUE" "$S_PAYOUT"
    if [ "$J_PAYOUT" -lt "$J_SUM" ]; then
        say "  ok   the junior absorbs the loss: $((J_PAYOUT - J_SUM)) against $J_SUM principal"
    else
        fail "the junior did not lose in a loss epoch: $J_PAYOUT against $J_SUM"
    fi
    expect_eq "the whole vault loss landed on the junior" "$GAIN" \
        "$(((S_PAYOUT - S_SUM) + (J_PAYOUT - J_SUM)))"
    say "  the vault lost $GAIN. The senior still received $((S_PAYOUT - S_SUM)) above"
    say "  principal, so the whole loss came out of the junior."
fi

# --- claim -------------------------------------------------------------------

step "claiming both tranches"
send_tx "claim_senior" "$SENIOR_IDENTITY" "$MANAGER_ID" -- claim \
    --claimant "$SENIOR_IDENTITY" --tranche Senior
send_tx "claim_junior" "$JUNIOR_IDENTITY" "$MANAGER_ID" -- claim \
    --claimant "$JUNIOR_IDENTITY" --tranche Junior

EPOCH=$(read_retrying "the claimed epoch" "$MANAGER_ID" current_epoch) ||
    die "could not read the epoch back after claiming"
S_PAID=$(json_field "$EPOCH" senior_paid)
J_PAID=$(json_field "$EPOCH" junior_paid)

# One depositor per tranche, so each is that tranche's last claimer and is paid
# the remainder rather than a pro-rata floor (spec section 8). Either way the
# tranche must pay out exactly its own payout, which is invariant 1 at the
# depositor level.
expect_eq "the senior tranche paid out in full" "$S_PAYOUT" "$S_PAID"
expect_eq "the junior tranche paid out in full" "$J_PAYOUT" "$J_PAID"
expect_eq "no senior principal left unclaimed" "0" "$(json_field "$EPOCH" senior_unclaimed)"
expect_eq "no junior principal left unclaimed" "0" "$(json_field "$EPOCH" junior_unclaimed)"
expect_eq "invariant 1 at the depositor level: senior + junior == V" "$V" "$((S_PAID + J_PAID))"

step "closing the epoch"
send_tx "close_epoch" "$ADMIN_IDENTITY" "$MANAGER_ID" -- close_epoch
EPOCH=$(read_retrying "the closed epoch" "$MANAGER_ID" current_epoch) ||
    die "could not read the epoch back after closing"
expect_eq "epoch status" "Closed" "$(json_str "$EPOCH" status)"

step "final depositor token balances"
SENIOR_BAL=$(read_scalar "the senior's balance" "$ASSET_ID" balance --id "$SENIOR_ADDR") ||
    die "could not read the senior's balance"
JUNIOR_BAL=$(read_scalar "the junior's balance" "$ASSET_ID" balance --id "$JUNIOR_ADDR") ||
    die "could not read the junior's balance"
say "  senior $SENIOR_IDENTITY $SENIOR_ADDR"
say "    balance $SENIOR_BAL stroops   (deposited $S_SUM, was paid $S_PAID)"
say "  junior $JUNIOR_IDENTITY $JUNIOR_ADDR"
say "    balance $JUNIOR_BAL stroops   (deposited $J_SUM, was paid $J_PAID)"

# --- summary -----------------------------------------------------------------

say ""
say "transaction hashes, scenario '$SCENARIO' on $NETWORK:"
for entry in "${TX_LOG[@]}"; do
    say "  $entry"
done

say ""
if [ "$FAILURES" -eq 0 ]; then
    say "scenario '$SCENARIO' PASSED - every figure matched docs/waterfall-spec.md."
    say "Unaudited testnet software. Do not use real funds."
    exit 0
fi
say "scenario '$SCENARIO' FAILED $FAILURES check(s). See the FAIL lines above."
exit 1