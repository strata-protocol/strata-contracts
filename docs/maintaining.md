# Maintaining Strata

How this repository is protected and released. For how to *contribute*, read
[`CONTRIBUTING.md`](../CONTRIBUTING.md).

**Strata is unaudited and testnet-only.** See
[`docs/risks.md`](risks.md) before changing anything that moves money.

---

## Branch protection

`main` was **unprotected** until this document and its script existed. Verified
against the API on 2026-10-06: `GET /repos/strata-protocol/strata-contracts/branches/main/protection`
returned `404 Branch not protected`, no repository rulesets existed, and neither
branch reported as `protected`.

`scripts/set-branch-protection.sh` sets the rules. **Dry run by default** — it
prints the exact `gh api` call and payload and changes nothing. Pass `--apply` to
make the call.

```bash
./scripts/set-branch-protection.sh              # print what it would do
./scripts/set-branch-protection.sh --apply      # do it
APPROVALS=2 ./scripts/set-branch-protection.sh # require two approvals instead of one
```

The script refuses to target any repository other than
`strata-protocol/strata-contracts`. The name is hardcoded rather than taken as an
argument on purpose: a script that accepts a repository name will eventually be
pointed at the wrong one by somebody in a hurry.

### What it sets

| Setting | Value | Why |
| --- | --- | --- |
| `required_pull_request_reviews` | on, `required_approving_review_count` = `$APPROVALS` (default 1) | nothing lands without a second pair of eyes |
| `dismiss_stale_reviews` | `true` | a review is invalidated by a later push |
| `require_code_owner_reviews` | `false` | there is no `CODEOWNERS` file yet |
| `required_status_checks.strict` | `true` | the branch must be up to date, not merely green at some earlier commit |
| `required_status_checks.contexts` | the five CI job names | see below |
| `required_conversation_resolution` | `true` | review threads get resolved, not abandoned |
| `allow_force_pushes` | `false` | no history rewriting on `main` |
| `allow_deletions` | `false` | `main` cannot be deleted |
| `enforce_admins` | **`false`** | see the next section |

### The required checks

The script reads the job names out of `.github/workflows/ci.yml` rather than
hardcoding them, because a hardcoded list silently stops protecting anything the
moment a job is renamed. They are:

| Job name | Guards |
| --- | --- |
| `fmt and clippy` | formatting, and lints with `-D warnings` |
| `test` | the whole suite, then again in release at 20 000 proptest cases |
| `build wasm` | a real `stellar contract build`, and that **both** named wasm artefacts exist |
| `docs are current` | every spec invariant has a property test, the spec's links resolve, the deploy script refuses non-testnets, the branch protection script's self-test passes |
| `dependency audit` | `cargo audit` |

**If you rename a job, re-run the script with `--apply`.** A required check that
no longer exists is not reported as missing — it simply stops being required.

### `dependency audit` may not actually block

That job is `continue-on-error: true`, so a new advisory does **not** fail the
workflow. It is listed as a required check anyway, because it should be. Whether
GitHub reports a `continue-on-error` job's check as passing when its steps fail
was **not verified** here — the job has only ever succeeded, so both behaviours
look identical. If you want it to genuinely block, either remove
`continue-on-error` from the job or accept that it is advisory. Your call; this
document does not decide it.

### Why admin enforcement is off

`enforce_admins: false` means an administrator can merge without the required
approval. That is a deliberate trade for a repository with one maintainer:

- GitHub does not count a review from the author of a pull request.
- So with `required_approving_review_count: 1` **and** admin enforcement on, a
  solo maintainer could not merge anything, ever, without a second person.

The cost is that the protection is advisory against the person who holds admin.
If a second maintainer is added, turn enforcement on:

```bash
gh api --method POST repos/strata-protocol/strata-contracts/branches/main/protection/enforce_admins
```

and re-run `set-branch-protection.sh` if the review count should change.

---

## The pull-request situation nobody has explained

Between 2026-10-05T13:08Z and 15:21Z, three pull requests were opened and merged
with no command-line action in the local clone: #1 and #2 from
`chore/testnet-deployment`, and #12 from `chore/wave-issues`. A fourth, #3, was
opened as `revert-1-chore/testnet-deployment` — a revert of #1 — and was closed
unmerged by the maintainer.

An earlier report in this repository claimed the repository had automation that
"auto-creates a PR when a `chore/*` branch is pushed". **That was wrong, and it
was inferred rather than verified.** It came from noticing a pull request whose
title was the branch name, at the same moment `gh pr create` returned a network
error.

What the evidence actually shows:

| Checked | Result |
| --- | --- |
| Workflows in this repository | exactly one, `.github/workflows/ci.yml` |
| Does `ci.yml` create PRs, merge, revert or push to `main`? | no — it has no `permissions:` block, no `gh` CLI call, and no write scope |
| Repository webhooks | none |
| Repository rulesets | none |
| Auto-merge on any PR | none; `autoMergeRequest` is null on all four |
| Who merged #1, #2, #12 | `sulaimonifeoluwa4-blip`, who is `ADMIN` on the repository |
| Merge commit shape | ordinary merge commits, not squash or rebase |
| PR #3's branch name | `revert-1-chore/testnet-deployment`, GitHub's own naming for the web UI's revert button |

The PR titles match the GitHub **web UI** conventions — a compare-branch form
titles the PR after the branch, and the revert button names the branch
`revert-<pr>-<branch>`. As a direct test: pushing to `chore/wave-issues` and to
`docs/wave-readiness` during this work created **no** pull request at all, and
the open-PR count stayed at zero.

**Conclusion: a human with admin access used the web UI.** There is no automation
in this repository that can merge without review.

**One thing could not be checked.** `gh api orgs/strata-protocol/actions/workflows`
returns `404` for this token, and `GET /repos/.../installation` returns `401`, so
organisation-level Actions workflows and installed GitHub Apps were **not
visible**. If the organisation has an Actions workflow or a GitHub App installed
at the org level, this investigation could not see it. A maintainer should check
Settings -> Actions and the org's installed apps to be sure.

**What this means for branch protection.** If a human is merging through the web
UI, required reviews will apply to them — an admin can still bypass, per the
section above. If something *does* exist at the org level, required reviews could
block it or be silently bypassed by it, and that is the single most important
thing for a maintainer to determine before relying on this protection.

**What to do if merges start appearing without local action again:** capture the
PR number and check `mergedBy` and the `author` of the pull request. Both are
recorded by the API and distinguish a human from an app.

---

## Releases

Nothing is tagged or released yet. `docs/release-notes-v0.1.0.md` is a **draft**
and carries the commands. The human runs them; an agent should not tag or
release.

```bash
gh release create v0.1.0 \
  --title "Strata v0.1.0 (unaudited, testnet only)" \
  --notes-file docs/release-notes-v0.1.0.md
```

## Testnet deployment upkeep

The deployment is live but **testnet is reset periodically by SDF**, which clears
contracts and accounts. When that happens the recorded IDs stop existing.

```bash
./scripts/verify-deployment.sh          # once issue #5 lands
./scripts/extend-ttl-testnet.sh         # extend the instances and Wasm
```

`extend-ttl-testnet.sh` keeps the contract instances and their Wasm alive. It does
**not** address risk R9 — a depositor's own position entry. That is issue #10 and
it is still open.

## Secrets

There are none in this repository, and there should never be. Testnet identities
for the scripts live in the Stellar CLI's own store, outside the working tree.
`.gitignore` covers `.env*` and `.stellar/`, and `.gitattributes` keeps shell
scripts at LF so a CRLF checkout cannot corrupt a shebang line.