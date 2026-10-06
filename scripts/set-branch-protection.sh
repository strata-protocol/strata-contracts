#!/usr/bin/env bash
# scripts/set-branch-protection.sh
#
# Sets (or prints) the branch protection rules for main on the Strata contracts
# repository. Dry run by default: nothing is changed unless --apply is passed.
#
# Usage:
#   ./scripts/set-branch-protection.sh                 # print the exact call
#   ./scripts/set-branch-protection.sh --apply         # apply it
#   ./scripts/set-branch-protection.sh --self-test     # no network, no gh needed
#   APPROVALS=2 ./scripts/set-branch-protection.sh     # require 2 approvals
#
# Requires: gh (GitHub CLI), authenticated, with admin rights on the repository.
#
# Why this is a script and not a setting: branch protection is invisible in the
# repository contents, so the exact configuration has to live somewhere reviewable.
# This file is that somewhere, and the dry run is the default so that reading it
# changes nothing.
#
# Strata is unaudited and testnet-only.

set -euo pipefail

# The one repository this script may touch. Hardcoded on purpose: a script that
# takes a repo name as an argument will eventually be pointed at the wrong one by
# somebody in a hurry, and branch protection is not something to get wrong.
REPO="strata-protocol/strata-contracts"
BRANCH="main"

# Number of approving reviews required. 0 disables the requirement entirely.
# 1 is the useful minimum: GitHub does not count a review from the author of the
# pull request, so 1 means "someone else has to look at this".
APPROVALS="${APPROVALS:-1}"

# The exact job names from .github/workflows/ci.yml, read from the file itself
# below rather than duplicated here. These are the strings GitHub shows in the
# checks list and therefore the strings branch protection has to match.
REQUIRED_CHECKS=()

APPLY=0
SELFTEST=0

die() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  sed -n '3,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# --- self-test ---------------------------------------------------------------
#
# Runs with no network, no gh, and no credentials, so CI can prove the guard and
# the payload shape without touching the repository. Everything here is a pure
# function of the arguments.

build_payload() {
  local checks_json
  checks_json=$(printf '%s\n' "${REQUIRED_CHECKS[@]}" | sed 's/.*/    "&"/' | paste -sd, - | sed 's/^/[/; s/$/]/')
  cat <<JSON
{
  "required_status_checks": {
    "strict": true,
    "contexts": $checks_json
  },
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "dismissal_restrictions": {},
    "dismiss_stale_reviews": true,
    "require_code_owner_reviews": false,
    "required_approving_review_count": $APPROVALS
  },
  "restrictions": null,
  "required_linear_history": false,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "block_creations": false,
  "required_conversation_resolution": true,
  "lock_branch": false,
  "allow_fork_syncing": true
}
JSON
}

selftest() {
  local failures=0
  check() {
    local what="$1" cond="$2"
    if [ "$cond" = "ok" ]; then
      echo "  ok   $what"
    else
      echo "  FAIL $what"
      failures=$((failures + 1))
    fi
  }

  # The guard is the whole point of hardcoding the repository name.
  local other
  for other in sulaimonifeoluwa4-blip/strata-contracts strata-finance/strata-contracts \
    strata-protocol/strata-protocol other/strata-contracts; do
    if [ "$other" = "$REPO" ]; then
      check "refuses nothing: '$other' was accepted" "FAIL"
    else
      check "refuses '$other'" "ok"
    fi
  done
  check "accepts '$REPO'" "$([ "$REPO" = "strata-protocol/strata-contracts" ] && echo ok || echo FAIL)"

  # The payload has to name every CI job, and must not enable admin enforcement,
  # because a solo maintainer cannot otherwise merge their own work.
  local payload
  payload="$(build_payload)"
  local check_name
  for check_name in "fmt and clippy" "test" "build wasm" "docs are current" "dependency audit"; do
    if printf '%s' "$payload" | grep -qF "\"$check_name\""; then
      check "payload requires the '$check_name' job" "ok"
    else
      check "payload requires the '$check_name' job" "FAIL"
    fi
  done
  check "payload requires branches to be up to date" \
    "$(printf '%s' "$payload" | grep -q '"strict": true' && echo ok || echo FAIL)"
  check "payload requires a pull request" \
    "$(printf '%s' "$payload" | grep -q 'required_pull_request_reviews' && echo ok || echo FAIL)"
  check "payload leaves admin enforcement off" \
    "$(printf '%s' "$payload" | grep -q '"enforce_admins": false' && echo ok || echo FAIL)"
  check "payload blocks force pushes" \
    "$(printf '%s' "$payload" | grep -q '"allow_force_pushes": false' && echo ok || echo FAIL)"
  check "payload blocks branch deletion" \
    "$(printf '%s' "$payload" | grep -q '"allow_deletions": false' && echo ok || echo FAIL)"
  check "payload requires resolved conversations" \
    "$(printf '%s' "$payload" | grep -q '"required_conversation_resolution": true' && echo ok || echo FAIL)"
  check "payload asks for $APPROVALS approval(s)" \
    "$(printf '%s' "$payload" | grep -q "\"required_approving_review_count\": $APPROVALS" && echo ok || echo FAIL)"

  # The payload has to be valid JSON, or GitHub answers 422 and the maintainer
  # is left guessing.
  #
  # Each candidate is probed by actually running it rather than by `command -v`.
  # On Windows, `python3` can resolve to the Microsoft Store stub, which is on
  # PATH and exits non-zero without doing anything; checking for existence alone
  # would then report a perfectly good payload as invalid.
  local json_ok="skip"
  local py
  for py in python3 python; do
    command -v "$py" >/dev/null 2>&1 || continue
    if printf '{"probe":1}' | "$py" -c 'import json,sys; json.load(sys.stdin)' >/dev/null 2>&1; then
      if printf '%s' "$payload" | "$py" -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
        json_ok="ok"
      else
        json_ok="FAIL"
      fi
      echo "  --   JSON check used: $py"
      break
    fi
  done
  if [ "$json_ok" = "skip" ]; then
    echo "  --   payload JSON check skipped: no working python on PATH"
  else
    check "payload is valid JSON" "$json_ok"
  fi

  [ "$failures" -eq 0 ] || {
    echo "self-test failed: $failures check(s)"
    exit 1
  }
  echo "self-test passed"
}

for arg in "$@"; do
  case "$arg" in
    --apply) APPLY=1 ;;
    --self-test) SELFTEST=1 ;;
    --help | -h)
      usage
      exit 0
      ;;
    *) die "unknown argument '$arg'. See --help." ;;
  esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CI_FILE="$ROOT_DIR/.github/workflows/ci.yml"

# The job names are read out of the workflow rather than hardcoded, because a
# hardcoded list silently stops protecting anything the moment a job is renamed.
# `name:` is the label GitHub shows in the checks list and therefore the string
# branch protection has to match.
if [ -f "$CI_FILE" ]; then
  while IFS= read -r job; do
    [ -n "$job" ] && REQUIRED_CHECKS+=("$job")
  done < <(sed -n 's/^    name: //p' "$CI_FILE" | grep -v '^$')
fi

if [ "${#REQUIRED_CHECKS[@]}" -eq 0 ]; then
  # Only fatal when we actually need to talk to GitHub; the self-test supplies
  # its own list so it can run without a checkout.
  if [ "$SELFTEST" -eq 1 ]; then
    REQUIRED_CHECKS=("fmt and clippy" "test" "build wasm" "docs are current" "dependency audit")
  else
    die "no job names found in $CI_FILE. Branch protection needs the exact job names."
  fi
fi

if [ "$SELFTEST" -eq 1 ]; then
  selftest
  exit 0
fi

case "$APPROVALS" in
  '' | *[!0-9]*) die "APPROVALS must be an integer between 0 and 6, got '$APPROVALS'" ;;
esac
[ "$APPROVALS" -le 6 ] || die "APPROVALS must be 0-6 (the API maximum), got $APPROVALS"

command -v gh >/dev/null 2>&1 || die "gh (GitHub CLI) is not installed"

TARGET="repos/$REPO/branches/$BRANCH/protection"
PAYLOAD="$(build_payload)"

echo "Target:      $REPO (branch: $BRANCH)"
echo "Approvals:   $APPROVALS"
echo "Checks:      ${REQUIRED_CHECKS[*]}"
echo
echo "The call that --apply would make:"
echo
echo "  gh api --method PUT $TARGET --input - <<'JSON'"
printf '%s\n' "$PAYLOAD"
echo "JSON"
echo

if [ "$APPLY" -ne 1 ]; then
  echo "DRY RUN. Nothing was changed."
  echo "Re-run with --apply to make this call."
  echo
  echo "To see what is currently set:"
  echo "  gh api $TARGET"
  exit 0
fi

if ! gh auth status >/dev/null 2>&1; then
  # Not a blocker on its own: `gh auth status` fails when any *other* account in
  # the local store is bad. The real question is whether this token works.
  echo "note: 'gh auth status' reported a problem; continuing and letting the call decide."
fi

echo "Applying..."
gh api --method PUT "$TARGET" --input - <<<"$PAYLOAD" >/dev/null ||
  die "the API call failed. Common causes: the token lacks admin rights on $REPO, or branch protection is unavailable on this plan."

echo "Applied. Verifying by reading it back:"
gh api "$TARGET" --jq '{
  strict: .required_status_checks.strict,
  contexts: .required_status_checks.contexts,
  approvals: .required_pull_request_reviews.required_approving_review_count,
  dismiss_stale_reviews: .required_pull_request_reviews.dismiss_stale_reviews,
  require_conversation_resolution: .required_conversation_resolution.enabled,
  enforce_admins: .enforce_admins.enabled,
  allow_force_pushes: .allow_force_pushes.enabled,
  allow_deletions: .allow_deletions.enabled
}'

cat <<'NOTE'

Note the interaction with admin enforcement:

  enforce_admins is false, so an administrator can still merge without the
  required approval. That is deliberate — see docs/maintaining.md. With
  approvals=1 and a single maintainer, GitHub will not count the author's own
  review, so without this a solo maintainer could not merge anything at all.

  To close that, enable admin enforcement (Settings -> Branches, or
  `gh api --method POST $TARGET/enforce_admins`), and accept that you will need
  a second approver.
NOTE