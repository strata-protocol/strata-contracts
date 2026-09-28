<!--
Thanks for contributing. Please fill this in. See CONTRIBUTING.md for ground rules.

Do not open a draft PR that changes docs/waterfall-spec.md. Spec changes happen
in a `spec` issue, and a maintainer lands the spec edit separately from the code.
-->

## What this changes

<!-- One or two sentences. If the diff is large, is it really one logical change? -->

## Anchored to

<!--
Every PR against the settlement math must name the spec clause or invariant it
is checked against. If you cannot fill this in, the change is probably out of scope.
-->

- Spec clause: <!-- e.g. "Waterfall spec, senior_due" or "n/a" -->
- Invariant: <!-- 1-5, or "n/a" -->

## How this was tested

- [ ] `cargo test` passes
- [ ] `cargo clippy --all-targets -- -D warnings` is clean
- [ ] `cargo fmt --check` is clean
- [ ] Added a test that fails without this change
- [ ] If the math changed: added or updated a **property** test, not just
      hand-picked cases

## Checklist

- [ ] Testnet only. Nothing here enables a mainnet deployment.
- [ ] No placeholder code presented as finished. Unfinished work is an issue.
- [ ] New storage entries, events, and error variants are documented in
      [`docs/architecture.md`](docs/architecture.md).
- [ ] New risks are recorded in [`docs/risks.md`](docs/risks.md).
- [ ] Unresolved work is linked as a GitHub issue, not left as a TODO comment.
- [ ] Commit messages follow `type(scope): imperative summary`.

## Wave points

<!-- The `drips:N` label on the linked issue. Confirm the scope did not grow. -->

- [ ] `drips:1` (1 point)
- [ ] `drips:3` (3 points)
- [ ] `drips:5` (5 points)
- [ ] `drips:8` (8 points)
