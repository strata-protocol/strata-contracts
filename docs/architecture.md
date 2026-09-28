# Architecture

How Strata is put together, and why each piece is shaped the way it is.
Settlement arithmetic is specified in
[waterfall-spec.md](waterfall-spec.md); this document covers everything around
it.

## Crates

```text
strata-contracts
├── contracts/
│   ├── waterfall          pure settlement math. no Env, no storage, no SDK.
│   ├── vault-interface    the ERC-4626 subset Strata needs + generated client
│   ├── epoch-manager      the contract. owns money and the state machine.
│   └── mock-vault         test double with an admin-set signed yield rate
└── tests/                 property tests + cross-contract integration tests
```

The dependency graph is strictly layered and acyclic:

```text
                    ┌──────────────────┐
                    │    waterfall     │  no dependencies at all, not even
                    │  (pure i128 math)│  soroban-sdk
                    └────────┬─────────┘
                             │
                    ┌────────┴─────────┐        ┌───────────────────┐
                    │  epoch-manager   │───────▶│  vault-interface  │
                    └────────┬─────────┘        └─────────┬─────────┘
                             │                            │
                             │                   ┌────────┴─────────┐
                             └───────────────────▶│   mock-vault     │
                                                 └──────────────────┘
```

`waterfall` deliberately has **no `soroban-sdk` dependency**. All of Strata's
money-handling logic is integer arithmetic on `i128`, so keeping it in a plain
Rust crate means the property tests need no ledger, no `Env` and no `testutils`
feature, and run in milliseconds. It also means there is exactly one
implementation of the split, and the contract cannot quietly disagree with it.

`vault-interface` exists so `epoch-manager` can depend on *an interface* rather
than on a particular vault. That is the whole meaning of "wraps any
ERC-4626-style vault". `mock-vault` implements it; so would a real vault.

## Data flow

```text
   depositor                epoch-manager              mock-vault          SAC
       │                        │                          │                │
       │ deposit(from, ...)     │                          │                │
       │───────────────────────▶│                          │                │
       │  require_auth(from)    │                          │                │
       │                        │──transfer(from, mgr)────┼───────────────▶│
       │                        │                          │                │
       │                        │  authorize_as_current_   │                │
       │                        │  contract(token.transfer)│                │
       │                        │──deposit(assets, mgr)───▶│                │
       │                        │                          │──transfer ─────▶│
       │                        │◀────── shares ───────────│                │
       │                        │                          │                │
       │            ... term passes, vault yield accrues ...                 │
       │                        │                          │                │
       │ settle()  (anyone)    │                          │                │
       │───────────────────────▶│                          │                │
       │                        │◀── balance_of(mgr) ──────│                │
       │                        │──redeem(all, mgr, mgr)──▶│                │
       │                        │                          │──transfer ─────▶│
       │                        │◀───────── V ─────────────│                │
       │                        │                          │                │
       │                        │ waterfall::settle(S,J,r,t,V)                │
       │                        │  -> senior_payout, junior_payout           │
       │                        │                          │                │
       │ claim(who, tranche)    │                          │                │
       │───────────────────────▶│                          │                │
       │  require_auth(who)     │                          │                │
       │                        │──transfer(mgr, who)──────┼───────────────▶│
       │◀───────────────────────│                          │                │
```

## The state machine

```text
(no epoch key)
      │ create_epoch(term, rate, ratio_bps)          [admin only]
      ▼
   ┌────────┐   deposit(from, tranche, amount)        [from only]
   │  Open  │──────────────────────────────────┐
   └────────┘                                  │ now >= maturity_ts
      │                                       ▼
      │ settle()                    deposit refused: DepositsClosed
      │ [anyone]                    (no admin override, no close step)
      ▼
  ┌─────────┐   claim(who, tranche)               [who only]
  │ Settled │──────────────────────────────────┐
  └─────────┘                                  ▼
      │                        position cleared; final claim in a
      │ close_epoch() [anyone]  tranche absorbs the rounding remainder
      │   refused while any
      │   principal is unclaimed
      ▼
  ┌────────┐
  │ Closed │──── create_epoch is allowed again
  └────────┘
```

Two properties of this shape are load-bearing:

- **Deposits close on a clock, not on an admin decision.** `deposit` refuses
  once `now >= maturity_ts`. There is no way for anyone to close the deposit
  window early or late, so the senior's exposure is a function of the epoch
  terms and nothing else.
- **A settled epoch with unclaimed money blocks the next epoch.** `create_epoch`
  refuses while `status != Closed`, and `close_epoch` requires every principal
  to be claimed. There is no path to a second epoch while funds from the first
  are outstanding.

## Storage layout

All entries are **persistent**, and all are TTL-bumped on every write
(`BUMP_THRESHOLD` / `BUMP_AMOUNT` in `epoch-manager`). A Soroban entry that
expires is a *silently missing* entry, and losing a depositor's position is
their money.

| Key | Type | Written by |
| --- | --- | --- |
| `Config` | `Config { admin, vault, asset }` | constructor only |
| `Epoch` | `Epoch { .. }` | every lifecycle transition |
| `Position(addr, tranche)` | `i128` principal | deposit, claim |

`Config` and `Epoch` are single structs rather than many keys so one TTL bump
covers each. Per-depositor positions are separate keys and are bumped
individually.

The `Epoch` struct holds both immutable configuration (terms, rate, ratio) and
live counters (totals, unclaimed, payouts). That is a deliberate trade: one
read gets a consistent snapshot, which matters because the claim path reads
`payout`, `paid`, `total` and `unclaimed` together and must not see them
half-updated.

**Key presence means "an epoch exists."** There is no stored `Option<Epoch>`.
`Storage::get` returns `Option<V>`, so a `-> Option<Epoch>` signature reading a
stored `Option<Epoch>` silently infers `V = Epoch` and then fails to decode a
stored `None`. Checking `has` first is both correct and readable.

## Authorisation

| Call | Authorised by | Notes |
| --- | --- | --- |
| `create_epoch` | `admin` | Only privileged operation in the contract. |
| `deposit` | `from` | The named depositor, not `env.invoker()`. |
| `claim` | `claimant` | Always for the named address. |
| `settle` | nobody | Permissionless once mature. |
| `close_epoch` | nobody | Permissionless, and can only move forward. |
| every view | nobody | Read-only. |

`from` and `claimant` are explicit parameters rather than `env.invoker()` so
that the authorisation scope visible in the transaction is exactly the scope
the contract checks. A position is **not transferable**, so there is no
allowance surface anywhere in the contract and no way to claim on someone
else's behalf.

### The one non-obvious authorisation

`deposit` pulls the depositor's tokens in, then locks them into the vault:

```text
manager → SAC.transfer(from, manager, amount)
manager → vault.deposit(amount, manager)
            └─▶ vault → SAC.transfer(manager, vault, amount)
```

The last call is **two frames** below the manager, and it needs the *manager's*
own authorisation. A contract's direct calls are implicitly authorised, but a
deeper call on its behalf is not, so the manager declares it up front:

```rust
env.authorize_as_current_contract(vec![&env, transfer_auth_entry(...)]);
```

Without the entry the call is rejected on chain. That is the correct failure
mode — a missing entry stops a deposit rather than permitting an unauthorised
pull.

`settle` and `claim` need no such entry: in `settle` the vault spends its own
shares and is the token source, and in `claim` the manager is the token source.

The integration test asserts the resulting auth tree directly. `mock_all_auths`
alone would pass even with a wrong entry, which is the trap the SDK docs warn
about.

## Interest accrual and the share price

Deposits go into the vault immediately rather than being pooled at maturity, so
a position earns yield from the moment it is made. Two consequences:

- The vault's **share price moves**, so a later depositor receives fewer shares
  for the same assets. The manager therefore tracks vault shares for
  redemption, but records **principal** for the position, because both the
  waterfall and the pro-rata split are defined over principal and principal
  does not move.
- The manager's position is a *pooled* vault position. Yield earned by one
  depositor's capital accrues to the pool and is shared according to the
  waterfall, not to the timing of individual deposits. This is a real
  characteristic of the design, not an oversight; see
  [risks.md](risks.md).

## The pro-rata split

```text
claim(who, tranche):
    payout     = tranche_payout          (fixed at settlement)
    total      = tranche_total           (principal deposited; never shrinks)
    mine       = principal(who, tranche)
    remaining  = tranche_unclaimed - mine

    amount = payout - already_paid    if remaining == 0     # last claim
           = payout * mine / total    otherwise            # pro-rata
```

Two details that are easy to get wrong, and were:

- The denominator is the tranche's **total** principal, not the unclaimed
  remainder. Dividing by a shrinking remainder gives every later claimant a
  larger slice than their share and overpays the tranche.
- The **last** claim in a tranche is paid the remainder instead of its pro-rata
  share, so the tranche pays out exactly `tranche_payout` in total and
  invariant 1 holds at the depositor level. The remainder is bounded by the
  dust from everyone's floor division, and is only reachable by a depositor who
  already holds a claimable position, so it is not griefable.

## Testing strategy

| Layer | Where | What it proves |
| --- | --- | --- |
| Spec arithmetic | `contracts/waterfall/src/tests.rs` | The worked examples and the boundaries, by name. |
| Invariants | `tests/src/waterfall_props.rs` | All five spec invariants, as properties over generated inputs. |
| Yield model | `contracts/mock-vault/src/test.rs` | The test double is trustworthy. |
| State machine | `contracts/epoch-manager/src/test.rs` | Lifecycle transitions, bounds, and who is allowed to do what. |
| Whole system | `tests/src/integration.rs` | A real manager, vault and token, end to end. |

The split is deliberate: the arithmetic is proved by property tests, the wiring
is proved by integration tests, and neither is asked to do the other's job.
Every settlement assertion in the integration tests is checked against
`strata_waterfall::settle` computed independently, so the contract's numbers
are never taken on trust.

## Extending this

- **A new underlying vault** implements
  [`VaultInterface`](https://docs.rs/strata-vault-interface) and nothing in
  `epoch-manager` changes.
- **New settlement math** goes in `waterfall`, with a spec change first, plus
  property tests for any new invariant.
- **New state** needs an entry in the storage table above and a decision about
  who may write it.
