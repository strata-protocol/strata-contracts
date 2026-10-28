# Architecture

How Strata is put together, and why each piece is shaped the way it is.
Settlement arithmetic is specified in
[waterfall-spec.md](waterfall-spec.md); this document covers everything around
it.

## Crates

```text
strata-contracts
├── contracts/
│   ├── waterfall              pure settlement math. no Env, no storage, no SDK.
│   ├── vault-interface        the ERC-4626 subset Strata needs + generated client
│   ├── epoch-manager          the contract. owns money and the state machine.
│   ├── mock-vault             test double with an admin-set signed yield rate
│   ├── blend-pool-interface   the Blend pool subset the adapter calls
│   ├── blend-adapter          vault-interface over a real Blend lending pool
│   └── blend-pool-mock        test double for a Blend pool. never deployed.
└── tests/                     property tests + cross-contract integration tests
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
ERC-4626-style vault". `mock-vault` implements it, and so does
`blend-adapter`, which is the worked example rather than a hypothetical:

```text
                            vault-interface
                                   │
                   ┌───────────────┴────────────────┐
          ┌────────┴─────────┐            ┌─────────┴──────────┐
          │    mock-vault    │            │   blend-adapter    │
          │  a rate an admin │            │  a real position   │
          │  types in        │            │  in a real pool    │
          └──────────────────┘            └─────────┬──────────┘
                                                    │
                                        ┌───────────┴────────────┐
                                        │ blend-pool-interface   │
                                        └───────────┬────────────┘
                                                    │
                              ┌─────────────────────┴───────────────────┐
                     ┌────────┴─────────┐                   ┌───────────┴──────────┐
                     │  blend-pool-mock │                   │  the Blend pool on   │
                     │  (a test double) │                   │  Stellar testnet     │
                     └──────────────────┘                   └──────────────────────┘
```

The adapter is layered the same way `epoch-manager` is: it depends on a pool
*interface*, so the test double and the real pool are interchangeable behind it.
`blend-pool-mock` is an `rlib` with no `cdylib`, so there is no Wasm artefact for
it and nothing that can mistake it for something deployable.

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

All entries are **persistent** (`env.storage().persistent()`). Unlike
`temporary()` storage — which is permanently deleted on expiry — a Soroban
`persistent()` entry whose TTL reaches zero is **archived** into the ledger's
BucketList and can always be restored by paying rent. Every write in
`epoch-manager` bumps the entry's TTL with `BUMP_THRESHOLD = 100_000` ledgers
(~5.79 days at 5 s/ledger) and `BUMP_AMOUNT = 4_000_000` ledgers (~231.48 days
at 5 s/ledger). See risk R9 in [risks.md](risks.md) for the full TTL audit
across a 365-day maximum epoch term (`6_307_200` ledgers).

| Key | Type | Written by | TTL extension |
| --- | --- | --- | --- |
| `Config` | `Config { admin, vault, asset }` | constructor only | bumped to `4_000_000` ledgers at construction |
| `Epoch` | `Epoch { .. }` | every lifecycle transition | bumped to `4_000_000` ledgers on every write (`create_epoch`, `deposit`, `settle`, `claim`, `close_epoch`) |
| `Position(addr, tranche)` | `i128` principal | deposit, claim | bumped to `4_000_000` ledgers on `deposit` and `claim` |

### Archival and restoring an expired entry with the CLI

If an epoch runs longer than `BUMP_AMOUNT` (`4_000_000` ledgers, ~231.48 days at
5 s/ledger) without a depositor touching their `Position(addr, tranche)` entry,
or if `Config` / contract instances go untouched past their TTL, the entry is
archived rather than deleted.

1. **Contract instances and Wasm** can be extended ahead of time or restored
   using the CLI:
   ```bash
   # Extend live instances and Wasm on testnet:
   ./scripts/extend-ttl-testnet.sh

   # Restore an archived contract instance or Wasm entry:
   stellar contract restore --id "$STRATA_MANAGER" --source-account strata-deployer --network testnet
   ```
2. **Per-depositor `Position(addr, tranche)` and `Config` / `Epoch` entries:**
   In Protocol 23+ (`stellar-cli` 28.x), transaction simulation for `claim` (or
   any write touching the key) automatically populates the restoration footprint
   when invoked with `--no-cache`:
   ```bash
   stellar contract invoke --id "$STRATA_MANAGER" --source-account strata-senior \
     --network testnet --no-cache -- claim --claimant strata-senior --tranche Senior
   ```
   A depositor can also explicitly restore an archived composite `DataKey` entry
   before claiming by passing its base64-encoded `ScVal` key (`--key-xdr`) to
   `stellar contract restore`:
   ```bash
   stellar contract restore \
     --id "$STRATA_MANAGER" \
     --key-xdr "<base64-xdr-of-DataKey-Position>" \
     --durability persistent \
     --source-account strata-senior \
     --network testnet
   ```

`blend-adapter` keeps its own entries, in the same style:

| Key | Type | Written by |
| --- | --- | --- |
| `Pool`, `Asset`, `Manager` | `Address` | constructor only, and never changed |
| `RecordedBTokens` | `i128` | after every call the pool answers |
| `TotalShares` | `i128` | deposit, redeem |
| `ShareBalance(addr)` | `i128` adapter shares | deposit, redeem |

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

## The Blend adapter

`contracts/blend-adapter` is a second implementation of `VaultInterface` whose
underlying is a real Blend v2 lending pool on Stellar testnet. The epoch manager
cannot tell it apart from `mock-vault`, which is the point of the interface
existing at all. The design and its consequences are researched in
[design/blend-adapter.md](design/blend-adapter.md); this section records what the
code actually does.

It holds one position per adapter, not one per depositor: it supplies the whole
epoch's assets to the pool in a single call, and redeems the whole position in a
single call. Its claim at the pool is a count of **bTokens**, whose value is
`floor(b_tokens * b_rate / 10^scalar)`. `b_rate` rises as borrowers pay interest
and falls when the pool socialises bad debt. `scalar` is read from the reserve
rather than assumed, because v2 scaled rates by 12 and v1 by 9.

### Events

Both carry the same three amounts: what moved, and the pool position afterwards.

| Event | Topics | Data |
| --- | --- | --- |
| `deposit_event` | the receiver | `assets` supplied, `shares` minted, `b_tokens` held |
| `redeem_event` | the owner, the receiver | `shares` burned, `assets` **actually paid**, `b_tokens` left |

The `assets` in `redeem_event` is what arrived, not what was asked for. It is the
number the epoch manager receives as `V` and the waterfall splits.

### Errors

The vault interface is fixed and its methods return values rather than
`Result`s, so every failure here is a trap with a code rather than a returned
error. `Error` discriminants are explicit, so the enum value *is* the on-chain
code.

| Code | Variant | Raised when |
| --- | --- | --- |
| 1 | `AlreadyInitialized` | the constructor ran twice |
| 2 | `ZeroAmount` | an amount is zero or negative |
| 3 | `ZeroShares` | a deposit would mint nothing for a real transfer |
| 4 | `InsufficientShares` | redeeming more shares than the ledger holds |
| 5 | `Unauthorized` | the owner is not the configured manager |
| 6 | `ReserveNotFound` | the pool does not list the asset at construction |
| 7 | `PositionMissing` | a position was recorded and the pool now reports none |
| 8 | `InsufficientWithdrawal` | the pool paid less than was asked for and said nothing |
| 9 | `SupplyNotRegistered` | a supply returned no position for the reserve |
| 10 | `ArithmeticOverflow` | a product or sum left `i128` |

### The second non-obvious authorisation

The epoch manager needed one declared entry because the token call sat two frames
below it. The adapter needs another, for the same reason:

```text
manager → adapter.deposit(assets, receiver)
            └─▶ adapter → SAC.transfer(manager, adapter, amount)   ← declared by the manager
                  adapter → pool.submit([Supply(asset, assets)])
                              └─▶ pool → SAC.transfer(adapter, pool, amount)  ← declared by the adapter
```

The adapter is the token's *source* on that second call and is not its invoker,
so without the entry the SAC rejects it. This was checked by removing it: every
test that supplies then fails with `Error(Auth, InvalidAction)`.

There is a limit to what a test can show here, and it is worth stating. With no
wildcard auth mocked, a successful deposit proves the entry is present and
correct — a wrong entry would be refused. But `env.auths()` reports the
authorisations supplied *to* the top-level invocation and does **not** surface
invoker-contract entries, so the adapter's own entry cannot be read back out of
the tree. The test asserts the tree the host demanded (the manager's `deposit`
and, under it, the transfer of the manager's own tokens) and relies on the call
succeeding for the adapter's half of it.

### A position that can expire

A Blend user entry that goes untouched for roughly 120 days reads back as
**empty**, not as an error, and Strata's maximum epoch term is 365 days. So the
adapter records the last position the pool reported and refuses to value or
redeem a position the pool has stopped recognising (`PositionMissing`) rather
than reporting it as worth zero — those two need different responses.

This is a detection, not a fix. Nothing keeps the entry alive yet, which is risk
B4 in [risks.md](risks.md).

## Testing strategy

| Layer | Where | What it proves |
| --- | --- | --- |
| Spec arithmetic | `contracts/waterfall/src/tests.rs` | The worked examples and the boundaries, by name. |
| Invariants | `tests/src/waterfall_props.rs` | All five spec invariants, as properties over generated inputs. |
| Yield model | `contracts/mock-vault/src/test.rs` | The test double is trustworthy. |
| State machine | `contracts/epoch-manager/src/test.rs` | Lifecycle transitions, bounds, and who is allowed to do what. |
| Pool model | `contracts/blend-pool-mock/src/test.rs` | The pool double is faithful where the adapter depends on it: rounding, the silent clamp, the utilisation gate, the empty read of an expired position. |
| Blend adapter | `contracts/blend-adapter/src/test.rs` | Every adapter function and every error, against that double, plus the auth tree for a deposit. |
| Adapter accounting | `tests/src/blend_adapter_props.rs` | Rounding cannot create value at a deposit or a redemption, a deposit/redeem round trip is bounded by principal plus accrued yield, and the share ledger moves only by the amounts calls report. |
| Whole system | `tests/src/integration.rs` | A real manager, mock vault and token, end to end. |
| Whole system, Blend | `tests/src/blend_adapter_integration.rs` | A real manager and real adapter through good, flat, loss and liquidity-limited epochs, with payouts recomputed from the spec. |

The split is deliberate: the arithmetic is proved by property tests, the wiring
is proved by integration tests, and neither is asked to do the other's job.
Every settlement assertion in the integration tests is checked against an
independently computed value — `strata_waterfall::settle` for the mock-vault
suite, and a direct transcription of spec §4–§5 for the Blend suite. In neither
suite is the contract's own figure used as its expected value.

## Extending this

- **A new underlying vault** implements
  [`VaultInterface`](https://docs.rs/strata-vault-interface) and nothing in
  `epoch-manager` changes. `blend-adapter` is a complete worked example of that,
  including the extra authorisation a vault which is itself a pool caller needs.
- **New settlement math** goes in `waterfall`, with a spec change first, plus
  property tests for any new invariant.
- **New state** needs an entry in the storage table above and a decision about
  who may write it.
