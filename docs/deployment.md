# Testnet deployment

**Strata is unaudited and testnet-only. Do not put real funds into it. Nothing
in this document has been reviewed by a third party.**

This document is how to reproduce the testnet deployment from nothing, and what
was actually deployed. Everything here was run on Stellar testnet on
2026-10-05; the contract IDs, wasm hashes and transaction hashes below are the
ones the network returned, not placeholders.

---

## What is deployed

| | |
| --- | --- |
| Network | `testnet` — `Test SDF Network ; September 2015` |
| Protocol version | 29 |
| Stellar CLI | 28.1.0 |
| soroban-sdk | 27.0.6 |
| Deployed at | 2026-10-05T11:35:50Z |
| Admin | `GBIMKBYJVP3VNFJPU45XMGBKHLESO5QLUYWAAMMRMGTPDF6NUD5OWRIP` |

| Contract | ID | Wasm sha256 | Bytes |
| --- | --- | --- | --- |
| `mock-vault` | [`CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6`](https://stellar.expert/explorer/testnet/contract/CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6) | `a3599c7022dc90c5df2fe009e49e25de6fb087536f872c588efc5cc732346759` | 9 638 |
| `epoch-manager` | [`CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU`](https://stellar.expert/explorer/testnet/contract/CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU) | `b12c8af8403df7478ee67f4e6dd292230236f086a4908894ef80d19417828f7a` | 25 964 |
| Native XLM SAC (the underlying) | [`CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`](https://stellar.expert/explorer/testnet/contract/CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC) | builtin | — |

The machine-readable record, written by the deploy script, is
[`deployments/testnet.json`](../deployments/testnet.json).

### The on-chain wasm hashes were checked, not assumed

`stellar contract info hash` returns the hash of the Wasm the ledger actually
holds. For both contracts it matched the sha256 of the locally built artefact
exactly, so the deployed code is the code in this repository at this commit.

### Why the wasm is smaller than the old hand-built numbers

The figures quoted before deployment were 46 565 bytes for `epoch-manager` and
24 229 for `mock-vault`. `stellar contract build` produces 25 964 and 9 638. The
difference is optimisation, not drift: the CLI runs wasm-opt over the output
(`--optimize`, on by default, and it sets `SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1`),
and it strips the contract specification into a custom section. A plain
`cargo build --target wasm32v1-none` does neither, which is why the README warns
that only `stellar contract build` produces deployable Wasm. `mock-vault` loses
the most because it carries a large spec for thirteen functions.

---

## Prerequisites

| Requirement | Version used | Notes |
| --- | --- | --- |
| Rust | 1.98.0 | pinned by `rust-toolchain.toml` |
| `wasm32v1-none` target | installed | `rustup target add wasm32v1-none` |
| Stellar CLI | 28.1.0 | prebuilt binary from the official releases page |
| `bash` | 5.x | the scripts are POSIX-ish bash |
| `sha256sum` or `shasum` | either | the deploy script needs one of them |

soroban-sdk 27.0.6 built and ran correctly under CLI 28.1.0 against a protocol
29 testnet. That is not the usual pairing — the CLI ships `stellar-xdr` 28.0.0
while the contracts pin SDK 27 — so it is worth stating plainly: it was
verified empirically, by building and by deploying and settling real epochs, and
it worked. It is not a claim that every SDK/CLI combination does.

Testnet closes a ledger every **5 seconds**, measured from consecutive Horizon
ledgers. Every epoch below is budgeted around that.

---

## Reproducing it from scratch

```bash
git clone https://github.com/strata-protocol/strata-contracts
cd strata-contracts
rustup target add wasm32v1-none

# The network guard is checkable with no network and no credentials. CI runs it.
./scripts/deploy-testnet.sh --self-test
```

### 1. Identities

```bash
for who in strata-deployer strata-senior strata-junior; do
  stellar keys generate --network testnet --fund "$who"
done
stellar keys ls
```

`--fund` uses Friendbot, which credits 10 000 XLM. Keys go into the Stellar
CLI's own store (`~/.config/stellar/identity/*.toml` on Linux and macOS,
`%USERPROFILE%\.config\stellar\identity\*.toml` on Windows), which is **outside
the repository**. Nothing in this repository reads or writes a key file, and
`.gitignore` covers `.stellar/` and `.env*` for the cases where someone does
create one. No secret was printed, logged or committed during this deployment.

### 2. Deploy

```bash
./scripts/deploy-testnet.sh strata-deployer testnet
```

The script builds the contracts, resolves the token, deploys `mock-vault` then
`epoch-manager`, writes `deployments/testnet.json` and prints the IDs. It refuses
any network outside `testnet`, `local`, `futurenet`, and the guard runs before
anything else so that asking for mainnet fails on the network regardless of what
else is wrong with the invocation.

Re-running deploys **new** contract IDs, so the script refuses to overwrite an
existing `deployments/testnet.json` unless given `--force`. Silently replacing
the record of a live deployment with a newer one is how a deployment becomes
unrecorded.

Only `testnet` was used. Nothing was deployed to mainnet, and the repository has
no mainnet code path.

### 3. Run the scenarios

```bash
./scripts/run-epoch-demo.sh good
./scripts/run-epoch-demo.sh loss
```

Each runs one complete epoch — open, deposit, lock, accrue, mature, settle,
claim, close — and checks every figure against
[`waterfall-spec.md`](waterfall-spec.md). See
[`demo-runbook.md`](demo-runbook.md) for a screen-recorded walkthrough.

### 4. Keep the deployment alive

```bash
./scripts/extend-ttl-testnet.sh
```

Extends the contract instances and their Wasm. This is a backstop for an idle
deployment; it is **not** the TTL audit, and it does not close risk R9.

---

## The underlying token, and why

The integration tests register a Stellar Asset Contract and mint to the
participants. There is no way to mint on a live network, so the demo has to use a
token that already exists and can be moved.

**Chosen: the native XLM Stellar Asset Contract**, 7 decimals, contract id
`CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`.

- It mirrors the tests, which use a SAC and never involve a classic issuer.
- No issuer account and no `changeTrust` is needed before a G-account can hold
  it, which keeps a demo to one deploy step.
- The alternative — a classic asset issued by a funded issuer and wrapped via
  `stellar contract asset deploy` — needs every depositor to create a trustline
  first, and a trustline is a moving part that has nothing to do with what is
  being demonstrated.
- Amounts are `i128` in the smallest unit, so 1 XLM is `10000000` stroops. The
  scripts deposit `1000000000`, which is 100 XLM.

The id is derived from the network rather than written into the script:

```bash
stellar contract id asset --asset native --network testnet
```

so it cannot be copied from the wrong network. `decimals` was read off the
deployed contract and returned `7`.

### The strategy reserve, and why the vault is pre-funded

`mock-vault`'s yield is unbacked accounting. `total_assets` grows with a positive
rate, but no assets arrive, so a redemption of an accrued gain fails for lack of
balance unless the vault already holds enough tokens to pay.

The integration tests solve this with a pre-funded strategy reserve, and so does
the demo: `run-epoch-demo.sh` tops the vault up to 50 XLM before each scenario
and says so in its output. The reserve is **not** counted in `total_assets`, so
it never disturbs the settlement figures. What it does mean is that the mock
vault's real token balance is not a meaningful assertion target — assert on
`total_assets` and on redemption values, as the tests do.

This is risk R7 in [`risks.md`](risks.md) and it is a property of the test
double, not of the wrapper.

---

## The two scenarios, and what the numbers prove

Both scenarios use the same terms, chosen so the arithmetic is visible inside a
short demo:

| Parameter | Value | Why |
| --- | --- | --- |
| term | 300 s | deposits need ~4 ledger closes; 5 minutes is comfortably enough |
| senior target rate | 10 000 bps/yr | the manager's documented ceiling |
| senior/junior ratio | 10 000 bps | 1.0x, so `S <= J` |
| vault yield | ±100 000 bps/yr | the mock's documented ceiling and floor |
| per-tranche deposit | 1 000 000 000 stroops | 100 XLM each |

With `S = J = 1_000_000_000`, `r = 10_000` and `t = 300`:

```
senior_interest I = (S * r * t) / (10_000 * 31_536_000) = 9_512
senior_due        = S + I                                 = 1_000_009_512
cushion           = J - I                                 =   999_990_488
```

The senior deposit sits exactly on the gate's cap, so `senior_room` goes to zero
and the buffer is exactly as large as the senior.

### `good` — the vault gains, the senior is capped

| | |
| --- | --- |
| `V` redeemed | 2 000 204 528 |
| `senior_due` | 1 000 009 512 |
| `senior_payout` | 1 000 009 512 |
| `junior_payout` | 1 000 195 016 |

`senior_payout + junior_payout == V` exactly. The vault gained 204 528 over the
two deposits' 2 000 000 000. The senior took **9 512** of it, which is exactly
its target and no more; the junior took the remaining **195 016**. That is the
cap binding, which is the entire point of a senior tranche.

| Step | Transaction |
| --- | --- |
| reserve top-up | [`da01f121…`](https://stellar.expert/explorer/testnet/tx/da01f121c4e591f1745a7129a7ee54ef573a14c388ecaafc3ce5786e4add8f71) |
| `create_epoch` | [`b5e0edaf…`](https://stellar.expert/explorer/testnet/tx/b5e0edafaa5d275ff2c4e0f4a7af669b6215d5d1b7054e040f5d9215fa3bca23) |
| junior deposit | [`64b1ab46…`](https://stellar.expert/explorer/testnet/tx/64b1ab46aa1f1addd3f71408487bbc68233bd866c93d5cdabeac0ef8b45364d1) |
| senior deposit | [`1f8927d9…`](https://stellar.expert/explorer/testnet/tx/1f8927d9df10b51344f92f49d40ef4630615866c19bcfeaa2c78526500804108) |
| `set_yield_rate` +100 000 | [`a0e77b3d…`](https://stellar.expert/explorer/testnet/tx/a0e77b3d89fd3b155a95707061981103a3792217d880caa24ad604e049adc7a4) |
| `settle` | [`7daa31e9…`](https://stellar.expert/explorer/testnet/tx/7daa31e9560110bd3a26d8b7e7b9686c5534781b4fa89d83d4d47da37fb8f27f) |
| senior claim | [`6e97573c…`](https://stellar.expert/explorer/testnet/tx/6e97573c7215d13ed0da09610d9e621c5ad6c4ce5d242fab79111124271ca328) |
| junior claim | [`0eb4db17…`](https://stellar.expert/explorer/testnet/tx/0eb4db17b37a4977d6b35e2006d8838c36f2d2c5565315ef70a63a4158b0094c) |
| `close_epoch` | [`d71f6781…`](https://stellar.expert/explorer/testnet/tx/d71f67817d1001c35ab72eef873b98043ccec9d30d6cd4018c73901664ddf3df) |

### `loss` — the vault loses, the junior pays for it

| | |
| --- | --- |
| `V` redeemed | 1 999 836 692 |
| `senior_due` | 1 000 009 512 |
| `senior_payout` | 1 000 009 512 |
| `junior_payout` | 999 827 180 |

Again `senior_payout + junior_payout == V` exactly. The vault lost 163 308. The
senior was made whole **and** still collected its 9 512 target, so the junior
absorbed 172 820 of its own 1 000 000 000 principal. Paid first, junior absorbs
first, in that order.

| Step | Transaction |
| --- | --- |
| reserve top-up | [`6a429b08…`](https://stellar.expert/explorer/testnet/tx/6a429b084ef758bdc5f1c04523ed328b010dd660c0fe0c63470fd15f70aed20f) |
| `create_epoch` | [`24c40dd7…`](https://stellar.expert/explorer/testnet/tx/24c40dd7eb7215ebbf2fb404b41d2a160ccc4b3a6ff69f595aadbbd0f55913d8) |
| junior deposit | [`b6017f60…`](https://stellar.expert/explorer/testnet/tx/b6017f6031c938a561da672febf4be466789f11a3e115ead7c8f6bd36b888f81) |
| senior deposit | [`98d9f4c3…`](https://stellar.expert/explorer/testnet/tx/98d9f4c39880cad2f35874eeebb0729cec84c0fa794ce0efdd5011d6fbe29973) |
| `set_yield_rate` −100 000 | [`bf28474a…`](https://stellar.expert/explorer/testnet/tx/bf28474af4115f275e798defcc1e5afffe3170d6ceb57253ae0dc4d172491917) |
| `settle` | [`e2e4ede7…`](https://stellar.expert/explorer/testnet/tx/e2e4ede76b452ea4570a34ef65a59b7c9eac40466e6956fa629d428ac1156a12) |
| senior claim | [`1ea4c02b…`](https://stellar.expert/explorer/testnet/tx/1ea4c02b569ffab6455181a2cc837a2bdd7949f72580ad50d6f4465ff791ea1c) |
| junior claim | [`14154412…`](https://stellar.expert/explorer/testnet/tx/14154412b3315c0b4fb7e60bc222e94860027585ba96f4fa999964fa1ab10714) |
| `close_epoch` | [`ea80028c…`](https://stellar.expert/explorer/testnet/tx/ea80028c3cf781a92a2e53bf493b35d9d22aba8a8f6cff3bd5c31d08a71a0c99) |

### The deposit gate is checked, not assumed

Both runs assert that a senior deposit of one stroop past the cap is refused,
and the run above logs `SeniorGateViolated`. `senior_room` is read before and
after each deposit and must go 1 000 000 000 → 0.

### The cushion boundary, through `project`

`project(value)` is the contract's own waterfall applied to an arbitrary `V`, so
the cushion cases can be checked against the deployed contract without needing a
real loss that large. Both scenarios check four points, and all four match the
spec computed independently in the script:

| `V` | `senior_payout` | `junior_payout` | meaning |
| --- | --- | --- | --- |
| 2 000 000 000 | 1 000 009 512 | 999 990 488 | break-even: the junior gets exactly its principal back |
| 1 000 009 512 | 1 000 009 512 | 0 | exactly at the cushion limit |
| 1 000 009 511 | 1 000 009 511 | 0 | one unit past it: the senior starts paying |
| 0 | 0 | 0 | total wipeout |

The first row is the one worth pausing on. At break-even the senior receives its
full target **and** the junior receives exactly `J`, which is spec §3's claim
that the senior's target interest is never funded out of junior principal.

---

## Known issues

### The CLI's simulation cache can produce an invalid footprint

**Observed with** Stellar CLI 28.1.0, soroban-sdk 27.0.6, protocol 29 testnet,
on 2026-10-05.

**Symptom.** A `deposit` on `epoch-manager` passed simulation — the CLI printed
`Signing transaction: …` — and then failed on submission with:

```
📔 CDWJS65B… - Failure - Log: {"vec":[{"string":"VM call trapped with HostError"},
  {"symbol":"deposit"},{"error":{"storage":"exceeded_limit"}}]}
❌ Error event: … = {"vec":[{"string":"trying to access contract data key
  outside of the footprint"},{"address":"CDWJS65B…"},{"vec":[{"symbol":"RateBps"}]}]}
```

The footprint the simulation returned did not cover the mock vault's `RateBps`
entry, which `accrue` reads on every interaction. The transaction was correctly
refused, so no funds were at risk; it simply could not be submitted.

**Workaround.** Pass `--no-cache`. The byte-identical transaction succeeded on the
first attempt with that flag:

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-senior \
  --network testnet --no-cache -- deposit --from strata-senior \
  --tranche Senior --amount 1000000000
```

`--no-cache` stops the CLI reusing a stored simulation. Every call in
`run-epoch-demo.sh` and `extend-ttl-testnet.sh` passes it. A script that runs a
long sequence of interacting transactions against one manager and one vault
should pass it throughout, because a stale footprint looks exactly like a
contract that refuses to work.

This is a CLI-side problem, not a defect in these contracts. It is recorded here
because it is the single most confusing failure hit during this deployment, and
the error text points at the contract rather than at the cache.

### Testnet RPC connections drop

Read-only calls intermittently fail with `client error (SendRequest)` or a TLS
handshake timeout. The network is fine and nothing happened; re-run the command.
The scripts here retry reads, but deliberately do **not** retry transaction
submission — a lost reply could otherwise file a duplicate. See the retry notes
in `run-epoch-demo.sh`.

---

## Known limits of this deployment

**A cushion-breaching loss cannot be demonstrated in a demo.** The largest loss
the mock can produce is `total_assets * 100_000bps * t / (10_000 * 31_536_000)`,
which is `t / 3_153_600` of the vault. The cushion is `J - I`, roughly `J` out of
`S + J = 2J`, so breaching it needs `t > 1_576_800` seconds — about **18.25 days**
— against a maximum term of 31 536 000 seconds. That is the contract's own bounds
being correct, not a script limitation, and the bounds were not changed. The
cushion cases above are therefore checked through `project` rather than by
settling an epoch that loses that much.

**The gain and loss in these demos are small in absolute terms.** At a 300-second
term even the maximum annualised rate only moves the vault by a fraction of a
percent. The arithmetic is exact regardless of magnitude and the invariants hold
to the stroop, but a viewer looking for a large number on screen will not find
one. Making the numbers large costs time, not correctness.

**`V` is read twice and differs.** The script records the vault's `total_assets`
before settling and then reads `value_redeemed` after. They differ by whatever
the intervening transactions accrued, because the mock accrues lazily against a
single global timestamp. The expectations are computed from the `V` that
settlement actually used.

**TTL is not audited.** `extend-ttl-testnet.sh` keeps the instances and Wasm
alive. Risk R9 — a depositor who deposits and never claims — is untouched and
still open.

**Two stray contracts exist on testnet from debugging this deployment**, neither
recorded in `deployments/testnet.json` and neither used by anything:

| Stray | What happened |
| --- | --- |
| `CCBFGA4E3UOI6DHN2PDYKXPJFIFKTLLIAG2GWX6SZGDXTSEFVTW6QDC7` | a trial `mock-vault` deployed by hand to learn the CLI's output format |
| `CBY632C7ENMRYHJ2IYDKYECUD6YTNTMPUVDUXHFCSWPQYYZQUJTIFXCH` | a second trial `mock-vault`, deployed by the first run of the deploy script before two of its bugs were fixed |

Neither has a manager pointing at it and neither ever held a deposit. They are
inert and safe to ignore; they are listed here so that nobody auditing testnet
account activity finds them unexplained.

**Testnet is not durable.** SDF resets testnet periodically — the next scheduled
reset is announced on the Stellar Dashboard — which clears accounts, contracts
and ledger entries. Nothing here is expected to survive one, and the contract IDs
above are only meaningful until then.