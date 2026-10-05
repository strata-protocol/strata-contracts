#!/usr/bin/env bash
# scripts/create-issues.sh
#
# Creates the labels and the first 8 Wave issues for strata-contracts.
#
# Usage:
#   ./scripts/create-issues.sh --dry-run            # print what would be created
#   ./scripts/create-issues.sh                      # create in the current repo
#   ./scripts/create-issues.sh --repo ORG/REPO      # create in a named repo
#
# Requires: gh (GitHub CLI), authenticated with `gh auth login`.
# Safe to re-run: an issue whose title already exists (open or closed) is skipped.
# Strata is unaudited and testnet-only.

set -euo pipefail

DRY_RUN=0
REPO=""

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --repo) shift; REPO="${1:-}" ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

die() {
  echo "error: $*" >&2
  exit 1
}

if ! command -v gh >/dev/null 2>&1; then
  echo "gh (GitHub CLI) is not installed" >&2
  exit 1
fi

# Ask the API, not `gh auth status`.
#
# `gh auth status` exits non-zero when *any* account in the store is bad, not
# only when the active one is. A machine can have one working account and two
# expired ones, in which case `gh auth status` fails while every real `gh`
# command succeeds. Gating on it produces the worst possible failure: the script
# refuses to run and tells the user to authenticate, when they already are.
#
# `gh api user` is the honest question — it only succeeds if the active
# credential can actually reach the API. A dry run needs no credential at all, so
# the check is skipped there.
if [ "$DRY_RUN" -eq 0 ]; then
  if ! ACTIVE_ACCOUNT="$(gh api user --jq .login 2>/dev/null)"; then
    echo "gh is not authenticated, or the active credential cannot reach the API." >&2
    echo "Check with: gh auth status" >&2
    echo "Then fix it with: gh auth login" >&2
    exit 1
  fi
fi

if [ -z "$REPO" ]; then
  REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
fi

echo "Target repo: $REPO"
[ "$DRY_RUN" -eq 1 ] && echo "DRY RUN: nothing will be created"

# ---------------------------------------------------------------- labels

# `gh label view` does not exist — `gh label` only has clone/create/delete/edit/
# list — so existence is checked against the full list. Getting this wrong is not
# cosmetic: a check that always fails reports every label as missing and claims
# the issues would be filed unlabelled when they would not.
# Retry a read-only listing call, keyed on the exit status and nothing else.
#
# Deliberately NOT keyed on whether the output is empty. A repository with no
# issues yet is a normal state, not an error, and a check that treats empty output
# as failure will retry five times and then refuse to run on exactly the
# repository this script exists to populate.
gh_retry_list() {
  local what="$1"
  shift
  local attempt out
  for attempt in 1 2 3 4 5; do
    if out="$("$@" 2>/dev/null)"; then
      printf '%s' "$out"
      return 0
    fi
    echo "  .. could not list $what (attempt $attempt/5), retrying" >&2
    sleep 5
  done
  return 1
}

EXISTING_LABELS="$(gh_retry_list labels gh label list --repo "$REPO" --limit 200 --json name --jq '.[].name')" ||
  die "could not read the existing labels from $REPO"

ensure_label() {
  local name="$1" color="$2" desc="$3"
  if [ "$DRY_RUN" -eq 1 ]; then
    if printf '%s\n' "$EXISTING_LABELS" | grep -Fxq -- "$name"; then
      echo "[label] $name  (exists, left alone)"
    else
      echo "[label] $name"
    fi
    return
  fi
  # An existing label is left exactly as it is. Three of these names
  # (`documentation`, `good first issue`, `help wanted`) are GitHub defaults that
  # already exist, and rewriting their colour and description would be a change
  # to existing repository metadata that nobody asked for.
  if printf '%s\n' "$EXISTING_LABELS" | grep -Fxq -- "$name"; then
    return
  fi
  if ! gh label create "$name" --repo "$REPO" --color "$color" --description "$desc" >/dev/null 2>&1; then
    echo "warning: could not create label '$name'; issues referring to it will be" \
      "filed without it" >&2
  fi
}

ensure_label "drips:1"           "0e8a16" "Wave points: small"
ensure_label "drips:3"           "fbca04" "Wave points: medium"
ensure_label "drips:5"           "f9a825" "Wave points: large"
ensure_label "drips:8"           "d93f0b" "Wave points: complex"
ensure_label "documentation"     "0075ca" "Docs only"
ensure_label "good first issue"  "7057ff" "Suitable for a first contribution"
ensure_label "help wanted"       "008672" "Maintainers want outside help"
ensure_label "scripts"           "c5def5" "Shell scripts and tooling"
ensure_label "testing"           "bfd4f2" "Tests and test infrastructure"
ensure_label "security"          "b60205" "Security analysis or hardening"
ensure_label "contracts"         "5319e7" "Touches Soroban contract code"

# ---------------------------------------------------------------- helpers

if [ "$DRY_RUN" -eq 1 ]; then
  EXISTING=""
else
  # Read once, before creating anything, and re-read on the next run to skip what
  # already landed. This is also what makes the script safe to re-run after a
  # failure partway through: titles already created are skipped, so a retry
  # cannot duplicate them.
  #
  # This call is retried because it goes through GraphQL, which on a flaky
  # connection fails with a TLS handshake timeout often enough to matter.
  EXISTING="$(gh_retry_list issues gh issue list --repo "$REPO" --state all --limit 500 --json title --jq '.[].title')" ||
    die "could not read the existing issue titles from $REPO"
fi

create_issue() {
  local title="$1" labels="$2" body="$3"

  if [ -n "$EXISTING" ] && grep -Fxq -- "$title" <<<"$EXISTING"; then
    echo "[skip]    already exists: $title"
    return
  fi

  if [ "$DRY_RUN" -eq 1 ]; then
    echo "[dry-run] $title"
    echo "          labels: $labels"
    return
  fi

  # Deliberately not retried. If the request succeeds on GitHub's side but the
  # reply is lost, a retry would file the same issue twice, and a duplicated
  # issue is harder to clean up than a failed run. Failing loudly and letting the
  # operator re-run is the safer trade, because the skip logic above then
  # catches whatever did land.
  local url
  if ! url="$(gh issue create --repo "$REPO" --title "$title" --label "$labels" --body "$body" 2>&1)"; then
    printf '%s\n' "$url" >&2
    echo "error: failed to create '$title'. Re-run the script; it will skip" \
      "anything already created." >&2
    exit 1
  fi
  echo "[created] $url"
}

read -r -d '' FOOTER <<'EOF' || true

---

**Ground rules**
- Strata is unaudited and testnet-only. Nothing here may add a mainnet path.
- `docs/waterfall-spec.md` is frozen. Do not change the math or the spec in a PR. If you think the spec is wrong, open an issue with the `spec` label.
- No placeholder code presented as finished. Anything you cannot finish becomes a new issue.
- Comment on this issue before you start so only one person works on it at a time.
- Read `CONTRIBUTING.md` first.
EOF

# ---------------------------------------------------------------- issue 1

read -r -d '' BODY <<'EOF' || true
## Summary

The README and docs use terms such as senior, junior, cushion, epoch, SAC, TTL and stroop without one place that defines them. Add `docs/glossary.md` so a new contributor can read the rest of the docs without outside research.

## Why it matters

Contributors arrive from different backgrounds (Rust, DeFi, Stellar). The cushion is `J - I`, not `J`, and `docs/risks.md` calls it the most misreadable property of the design. A short, accurate glossary lowers the cost of getting started and of reviewing.

## Acceptance Criteria

- [ ] `docs/glossary.md` exists and defines at least: senior, junior, tranche, epoch, waterfall, senior target rate, cushion, deposit gating ratio, underlying vault, Stellar Asset Contract (SAC), TTL, stroop, basis point
- [ ] Each definition is one to three sentences in plain English
- [ ] No definition makes a claim about contract behaviour that `docs/waterfall-spec.md` or `docs/architecture.md` does not already make
- [ ] The cushion entry states `J - I` (not `J`) and links to the relevant spec section
- [ ] `README.md` links to the glossary
- [ ] All markdown links resolve (the CI spec-consistency job checks this)

## Tech Stack

Markdown only. No code changes.

**Suggested Wave complexity:** Trivial
EOF
create_issue \
  "docs(glossary): add a glossary of tranche and Soroban terms" \
  "drips:1,documentation,good first issue" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 2

read -r -d '' BODY <<'EOF' || true
## Summary

`docs/deployment.md` says the on-chain wasm hashes were compared with the locally built artefacts by hand, using `stellar contract info hash`. Automate that check in `scripts/verify-deployment.sh` so anyone can confirm that the deployment recorded in `deployments/testnet.json` is live and matches the code in this repository.

## Why it matters

Testnet is reset periodically, so recorded contract IDs can stop existing. Reviewers and contributors need a one-command way to tell "deployment is gone" apart from "deployment does not match the repo".

## Acceptance Criteria

- [ ] `scripts/verify-deployment.sh` reads `deployments/testnet.json`
- [ ] For each recorded contract it fetches the on-chain wasm hash and compares it with the recorded sha256
- [ ] A `--rebuild` flag runs `stellar contract build` and also compares the local artefact hash
- [ ] It calls one read-only function per contract (a view on the epoch manager, `total_assets` on the mock vault) and fails if the call errors
- [ ] It exits non-zero with distinct messages for "contract not found" (for example after a testnet reset) and "hash mismatch"
- [ ] It refuses any network other than `testnet`, reusing the guard from `scripts/deploy-testnet.sh`
- [ ] A `--self-test` mode runs with no network and no credentials, and CI runs it
- [ ] `docs/deployment.md` documents the script

## Tech Stack

Bash, Stellar CLI. Use `jq` only if you add a check that it is installed.

**Suggested Wave complexity:** Medium
EOF
create_issue \
  "feat(scripts): add verify-deployment.sh to check deployments/testnet.json against the ledger" \
  "drips:3,scripts,testing,help wanted" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 3

read -r -d '' BODY <<'EOF' || true
## Summary

`contracts/waterfall/src/lib.rs` is the only place in the repository where money is divided. It is small and has no dependencies, but it was written and tested by the same author. Review it independently against `docs/waterfall-spec.md`.

## Why it matters

Four bugs that shipped were caught by tests (see the `[realised]` entries in `docs/risks.md`), including a documented limit that was 1000 times the real one. A reviewer who did not write the code is the most likely way to find the next one.

## Acceptance Criteria

- [ ] A review is published as `docs/reviews/waterfall-review-<date>.md`
- [ ] It checks each of the five spec invariants by reading the code and the property tests in `tests/src/waterfall_props.rs`, and says whether the tests would catch a violation
- [ ] It checks the rounding rule (the remainder goes to the junior), the behaviour at the `senior_due` boundary, and error mapping
- [ ] It recomputes the documented overflow limits independently and states whether they hold for `i128`
- [ ] It says plainly which parts the reviewer did not review
- [ ] Every finding is filed as its own issue with a reproduction (a failing test or worked numbers); disagreements with the spec carry the `spec` label
- [ ] The PR changes neither the crate nor the spec

## Tech Stack

Rust, `proptest`, reading `docs/waterfall-spec.md`.

**Suggested Wave complexity:** Medium
EOF
create_issue \
  "review(waterfall): independent review of contracts/waterfall against docs/waterfall-spec.md" \
  "drips:3,security,documentation,help wanted" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 4

read -r -d '' BODY <<'EOF' || true
## Summary

`docs/risks.md` lists network resource limits among the risks that have not been analysed. Measure CPU instructions, memory and ledger footprint for every epoch-manager entrypoint and report how close each one is to the network limits.

## Why it matters

A contract that works in a test environment can still exceed per-transaction resource limits on a live network, especially if cost grows with the number of depositors.

## Acceptance Criteria

- [ ] A test or harness under `tests/` runs each state-changing entrypoint and each read-only view and records CPU instructions, memory, and read/write footprint, using the cost-measurement tooling of the pinned `soroban-sdk` (check its docs for the current API; do not assume one)
- [ ] It repeats the measurements with 1, 10 and 100 positions to show whether any entrypoint's cost grows with the number of depositors
- [ ] Results are committed as `docs/resource-report.md`, with the SDK version, date and the exact command that reproduces them
- [ ] The report compares each result with the current Stellar network limits, citing the official page and the date it was checked, and says which entrypoints come close
- [ ] Any entrypoint whose cost scales with depositor count is filed as a separate issue
- [ ] `docs/risks.md` moves this risk from "not analysed" to analysed, with a link to the report

## Tech Stack

Rust, `soroban-sdk` test utilities.

**Suggested Wave complexity:** Medium
EOF
create_issue \
  "test(resources): measure CPU and memory budget for every epoch-manager entrypoint" \
  "drips:5,testing,contracts,help wanted" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 5

read -r -d '' BODY <<'EOF' || true
## Summary

The epoch manager calls an external vault (deposit, redeem, `total_assets`) and relies on it to behave. `docs/risks.md` lists hostile-vault reentrancy as not analysed. Find out what a malicious vault can and cannot do to the manager, and prove it with tests.

## Why it matters

Strata is designed to wrap any ERC-4626-style vault. Today only a trusted mock vault is used. Before any real vault adapter is built, the manager's behaviour against a hostile or broken vault must be known, not assumed.

## Acceptance Criteria

- [ ] A test-only malicious vault that implements the vault interface and tries to call back into the epoch manager (deposit, settle, claim) during its own `deposit` and `redeem`
- [ ] For each re-entry attempt the tests record the observed outcome (rejected by the host, rejected by the manager, or succeeded), with a link to the Soroban documentation on re-entrancy
- [ ] Additional vault behaviours are tested and recorded: inflated or deflated `total_assets`, an inflated or deflated `redeem` result, and a vault that traps; for each, say whether funds can become stuck and who can recover them
- [ ] Findings are added to `docs/risks.md` with a severity
- [ ] Anything exploitable is filed as a separate issue and is not fixed in this PR without maintainer agreement
- [ ] No change to `docs/waterfall-spec.md` or to payout math

## Tech Stack

Rust, `soroban-sdk` test utilities, `proptest` where useful.

**Suggested Wave complexity:** Medium
EOF
create_issue \
  "test(security): analyse hostile-vault reentrancy and add a malicious-vault test" \
  "drips:5,security,testing,contracts" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 6

read -r -d '' BODY <<'EOF' || true
## Summary

`docs/risks.md` lists settlement front-running as not analysed. For each state transition in the epoch lifecycle (create, deposit, lock, settle, claim), establish who can call it, which ledger time it reads, and whether transaction ordering around maturity lets anyone gain at someone else's expense.

## Why it matters

Settlement divides real money between two tranches. If the outcome depends on who submits first, that has to be known and documented before anyone relies on it.

## Acceptance Criteria

- [ ] `docs/analysis/settlement-ordering.md` lists, for each state transition: who can call it, the constraints it enforces, and the ledger time fields it reads
- [ ] These scenarios are checked with a test or with worked numbers: a deposit in the last ledger before lock; `settle` called by a third party at the first eligible ledger versus later; claim ordering between senior and junior; a mock-vault rate change just before `settle` (note that this is a testnet-only admin power)
- [ ] Each scenario states whether anyone gains or loses from ordering, and by how much
- [ ] The conclusion separates real risks, bounded risks, and artefacts of the mock vault
- [ ] `docs/risks.md` is updated, and anything exploitable is filed as a separate issue
- [ ] No change to `docs/waterfall-spec.md` or to payout math

## Tech Stack

Rust tests, Markdown. Testnet runs with `scripts/run-epoch-demo.sh` are optional supporting evidence.

**Suggested Wave complexity:** Medium
EOF
create_issue \
  "docs(security): analyse settlement ordering and front-running around maturity" \
  "drips:5,security,documentation,testing" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 7

read -r -d '' BODY <<'EOF' || true
## Summary

Positions are extended on write, but a depositor who never claims is left alone for `BUMP_AMOUNT` ledgers. This is risk R9 in `docs/risks.md`. Audit how long every storage entry stays alive against realistic ledger timings and the maximum epoch term, and fix or document what you find.

## Why it matters

If a position entry can expire between deposit and claim, a depositor's funds can become hard to reach. Testnet closes a ledger roughly every 5 seconds and the maximum epoch term is 31 536 000 seconds, so the numbers need to be worked out, not guessed.

R9 is one of several open Medium risks in `docs/risks.md` — R4, R5, R8, R10, R16 and R17a are also open at that severity — but it is the one where a number that has not been computed is load-bearing for whether a depositor can still get their money back. That makes it the cheapest of them to retire.

## Acceptance Criteria

- [ ] An inventory of every entry the manager and vault write, split by instance and persistent storage: when its TTL is set, when it is extended, by whom, and the `BUMP_AMOUNT` and threshold values currently in the code
- [ ] Those values converted to wall-clock time using the measured ledger close time, compared with the maximum epoch term plus a claim window, with a clear statement of whether a position can expire before it is claimed
- [ ] Tests that advance the ledger sequence past the TTL and show current behaviour (still live, archived, or needs restoration)
- [ ] If there is a gap: a fix proposal is posted on this issue first, and is implemented only after maintainer approval (for example extension sized to the epoch term, or a public keep-alive function), with tests; payout math and `docs/waterfall-spec.md` must not change
- [ ] Docs explain archival and how a depositor restores an archived entry with the CLI
- [ ] `docs/risks.md` R9 is updated with status and evidence

## Tech Stack

Rust, `soroban-sdk` ledger and TTL test utilities, Stellar CLI.

**Suggested Wave complexity:** High
EOF
create_issue \
  "fix(ttl): audit per-user position TTL and prove positions survive until claimed (R9)" \
  "drips:8,security,contracts" \
  "${BODY}${FOOTER}"

# ---------------------------------------------------------------- issue 8

read -r -d '' BODY <<'EOF' || true
## Summary

The admin can choose an epoch rate the underlying will not earn, and depositors have to trust those terms. This is risk R10 in `docs/risks.md`. Reduce unilateral admin power with a multisig admin, a timelock on parameter changes, or both.

## Why it matters

Strata's pitch is that the senior's protection is real. If one key can change the terms an epoch runs on, that protection depends on trusting one key.

## Acceptance Criteria

- [ ] A design note, `docs/design/admin-control.md`, is written and approved by a maintainer on this issue before any contract code: it compares the options (a multisig or custom-account admin, a timelock, enforced parameter bounds) with their trade-offs
- [ ] An inventory of every admin-only function in the epoch manager and the mock vault, and what each one can do to depositors
- [ ] The chosen design is implemented with tests: an unauthorised caller is rejected, the timelock delay is enforced, a queued change can be cancelled, and events are emitted
- [ ] Existing integration tests still pass, and payout math and `docs/waterfall-spec.md` are unchanged
- [ ] Docs and a script show how to set up the multisig admin on testnet
- [ ] `docs/risks.md` R10 is updated

## Tech Stack

Rust, `soroban-sdk` auth and custom accounts, Stellar CLI.

**Suggested Wave complexity:** High
EOF
create_issue \
  "feat(admin): design and implement multisig and timelock admin control (R10)" \
  "drips:8,security,contracts" \
  "${BODY}${FOOTER}"

echo
echo "Done."