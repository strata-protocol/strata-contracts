# Demo runbook

**Strata is unaudited and testnet-only. Do not put real funds into it.**

An ordered script for recording a walkthrough of a good epoch and a loss epoch
on Stellar testnet. Every command below is one that was actually run, and every
expected figure is one the network actually returned. See
[`deployment.md`](deployment.md) for the full write-up.

You cannot record this for the maintainer. This is the script to follow.

---

## Before you press record

Total wall clock is about **20 minutes**, most of it waiting.

```bash
stellar --version          # 28.1.0
stellar network ls --long  # confirm testnet's passphrase and RPC
stellar keys ls            # strata-deployer, strata-senior, strata-junior
```

Check the manager is idle. It allows one epoch at a time:

```bash
stellar contract invoke --id "$(grep -oE 'C[0-9A-Z]{55}' deployments/testnet.json | tail -1)" \
  --source-account strata-deployer --network testnet --send=no -- current_epoch
```

If `status` is anything other than `Closed`, the previous epoch has unclaimed
money and the demo cannot start. Claim both tranches and `close_epoch` first —
`run-epoch-demo.sh` prints the exact three commands when it refuses.

Put the terminal in a window where the contract IDs are visible. Everything below
assumes:

```bash
cd strata-contracts
export VAULT=CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6
export MANAGER=CB57H6NE7CIPHEDO2HJT7IX55NHP6RXI2EPSUIGK65NLG5CCXC4JB7QU
export ASSET=CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
```

If testnet has been reset since this deployment, the contracts are gone. Redeploy
first with `./scripts/deploy-testnet.sh strata-deployer testnet --force` and take
the new IDs from the output.

---

## Act 1 — what was deployed (1 minute, no waiting)

Show the record rather than describing it.

```bash
cat deployments/testnet.json
```

Point out, in this order:

- `"audited": false` and the warning line. Say it out loud.
- The two contract IDs and their wasm hashes.
- That the on-chain wasm matches what is in this repository:

```bash
stellar contract info hash --id "$VAULT" --network testnet
# a3599c7022dc90c5df2fe009e49e25de6fb087536f872c588efc5cc732346759
stellar contract info hash --id "$MANAGER" --network testnet
# b12c8af8403df7478ee67f4e6dd292230236f086a4908894ef80d19417828f7a
```

- The contracts are alive:

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet --send=no -- vault
# "CDWJS65BA26QBTA4L6LBX76B2XPGAQHY25USI6Z3MSQ73T5NQTXARSQ6"

stellar contract invoke --id "$VAULT" --source-account strata-deployer \
  --network testnet --send=no -- total_assets
# "0"
```

The manager is pointing at the vault, and the vault is empty. That is the whole
system at rest.

---

## Act 2 — the good epoch (about 6 minutes, 5 of them waiting)

The honest way to run this is the script, because the script is what checks the
numbers. Run it and narrate the output:

```bash
./scripts/run-epoch-demo.sh good
```

If you want to drive it by hand instead, these are the seven transactions in
order. **Do not do both** — the manager takes one epoch at a time.

**1. Open the epoch.** 300 seconds, senior target 10 000 bps/yr (the ceiling),
gate 10 000 bps (1.0x).

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet -- create_epoch \
  --term_seconds 300 --rate_bps 10000 --max_senior_ratio_bps 10000
```

> **Narrate:** the term is 300 seconds because that is short enough to watch and
> long enough for four transactions to land at testnet's 5-second ledger
> cadence. The senior rate and the ratio are both at their documented maxima.

**2. Junior deposit first — this is not a style choice.**

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-junior \
  --network testnet -- deposit --from strata-junior --tranche Junior \
  --amount 1000000000
```

> **Narrate:** 1 000 000 000 stroops is 100 XLM, because XLM has 7 decimals. The
> junior goes in *first* because the gate refuses senior principal until junior
> capital exists. Try the senior deposit now and watch it fail — this costs
> nothing, it is a simulation:
>
> ```bash
> stellar contract invoke --id "$MANAGER" --source-account strata-senior \
>   --network testnet --send=no -- deposit --from strata-senior --tranche Senior \
>   --amount 1
> # error: transaction simulation failed: HostError: Error(Contract, #10)
> ```
>
> `#10` is `SeniorGateViolated`. The buffer is real, not decorative.

**3. Senior deposit, exactly on the cap.**

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-senior \
  --network testnet -- deposit --from strata-senior --tranche Senior \
  --amount 1000000000

stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet --send=no -- senior_room
# "0"
```

> **Narrate:** `senior_room` went from 1 000 000 000 to 0. The senior now sits
> exactly on the cap, so the junior buffer is exactly as large as the senior
> principal.

**4. Set the vault's yield.**

```bash
stellar contract invoke --id "$VAULT" --source-account strata-deployer \
  --network testnet -- set_yield_rate --rate_bps=100000
```

> **Narrate:** +1000%/yr, the mock vault's documented ceiling. Note the
> `--rate_bps=100000` form with the equals sign — a negative value needs it.

**5. Wait out the term.** This is the five minutes. Poll:

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet --send=no -- seconds_to_maturity
```

> **Narrate while it counts down:** deposits closed automatically at maturity.
> Nobody closed them. There is no admin step and no way to close the window
> early, so the senior's exposure is a function of the terms and nothing else.

**6. Settle.**

```bash
stellar contract invoke --id "$VAULT" --source-account strata-deployer \
  --network testnet --send=no -- total_assets
# "2000171232"  — or near it; accrual is lazy, so it keeps moving

stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet -- settle
```

`settle` returns the split and prints the numbers that matter:

```
value_redeemed: "2000204528"
senior_due:      "1000009512"
senior_payout:   "1000009512"
junior_payout:   "1000195016"
```

> **Narrate — this is the payoff of the whole demo:**
>
> 1 000 009 512 + 1 000 195 016 = 2 000 204 528. Exactly the value redeemed.
> Nothing created, nothing lost.
>
> The vault gained 204 528. The senior took **9 512** of it. That is not a
> coincidence: 1 000 000 000 × 10 000 × 300 ÷ (10 000 × 31 536 000) = 9 512,
> which is the senior's target interest for a 300-second term at 100%/yr. The
> senior got its target and not one stroop more.
>
> The junior took the other **195 016**. That is what "senior" means here.

**7. Claim.**

```bash
stellar contract invoke --id "$MANAGER" --source-account strata-senior \
  --network testnet -- claim --claimant strata-senior --tranche Senior
# ClaimEvent ... amount: "1000009512"

stellar contract invoke --id "$MANAGER" --source-account strata-junior \
  --network testnet -- claim --claimant strata-junior --tranche Junior
# ClaimEvent ... amount: "1000195016"

stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet -- close_epoch
```

> **Narrate:** `settle` is permissionless and so is `close_epoch`, but the epoch
> cannot be closed while anyone is still owed money. Nobody can open a second
> epoch until every claim is made. There is no path around that.

---

## Act 3 — the loss epoch (about 6 minutes)

Identical flow, one sign changed.

```bash
./scripts/run-epoch-demo.sh loss
```

By hand, it is Act 2 again with
`--rate_bps=-100000` and `--rate_bps` written with the equals sign.

The numbers:

```
value_redeemed: "1999836692"
senior_due:      "1000009512"
senior_payout:   "1000009512"
junior_payout:   "999827180"
```

> **Narrate:**
>
> The vault lost 163 308. The senior was made whole anyway — 1 000 009 512, its
> full target — and the junior collected 999 827 180 against 1 000 000 000 of
> principal. The junior ate the whole 172 820 of the loss.
>
> Again the two payouts sum to `V` exactly.
>
> This is the asymmetry, and it is the entire design: same terms, same gate,
> opposite sign on the underlying, and the loss lands on the junior while the
> senior walks away whole.

---

## Act 4 — the cushion boundary (1 minute, no waiting)

The loss above is well inside the cushion, and that is not a choice — a real
cushion-breaching loss needs about 18.25 days at the contract's own maximum rate
and term bounds. So show the boundary with the contract's projection view
instead. It is the deployed contract answering for an arbitrary `V`.

```bash
# Break-even: the senior gets its target, the junior gets its principal back
stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet --send=no -- project --value 2000000000
# senior_due 1000009512  senior_payout 1000009512  junior_payout 999990488
```

> **Narrate:** the junior receives exactly `J`. Its principal is untouched. The
> senior's target interest is *not* funded out of the junior's money — it comes
> out of the vault's yield.

```bash
# One stroop below senior_due: the junior is wiped, the senior is untouched
stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet --send=no -- project --value 1000009511
# senior_payout 1000009511  junior_payout 0
```

> **Narrate:** one stroop of extra loss and the junior goes to zero while the
> senior is still whole. That is the cushion limit, and it is sharp.

```bash
# Total wipeout
stellar contract invoke --id "$MANAGER" --source-account strata-deployer \
  --network testnet --send=no -- project --value 0
# senior_payout 0  junior_payout 0
```

> **Narrate, and do not soften it:** in a wipeout the senior loses everything
> too. There is no floor under senior principal. "Senior" means paid first, not
> safe. That is risk R4 in [`risks.md`](risks.md) and it is still open.

---

## Act 5 — close honestly (30 seconds)

Leave this on screen.

```bash
git log --oneline -6
cat deployments/testnet.json | head -20
```

> **Narrate:** the settlement math is property-tested over five invariants and
> the whole suite is 104 tests, all green in CI. None of that is an audit. There
> has been no third-party review, no formal verification, and no independent
> reimplementation to diff against. This is unaudited testnet software and it
> should not hold real funds.

---

## Timing summary

| Act | Wall clock | Mostly waiting? |
| --- | --- | --- |
| 1 — what was deployed | ~1 min | no |
| 2 — good epoch | ~6 min | yes, 5 min of it |
| 3 — loss epoch | ~6 min | yes, 5 min of it |
| 4 — cushion boundary | ~1 min | no |
| 5 — close | ~0.5 min | no |
| **Total** | **~15 min** | |

## If something goes wrong mid-demo

**`deposit` fails with `Error(Contract, #10)`.** Expected if you try a senior
deposit before the junior, or one stroop over the cap. That is the gate working.

**A transaction fails on submission with `storage exceeded_limit`** and mentions
a key outside the footprint. Add `--no-cache` to that command and run it again.
The CLI caches simulations, and a cached one can carry a footprint that no longer
covers the current state. This was hit during the real deployment and it is why
`run-epoch-demo.sh` passes `--no-cache` on every call.

**A read fails with `client error (SendRequest)`.** Testnet's RPC drops
connections. Just run the command again; nothing happened.

**`create_epoch` fails with `EpochAlreadyActive`.** The previous epoch has
unclaimed money. Claim both tranches, then `close_epoch`.

**`settle` fails with `NotMature`.** Too early. Check `seconds_to_maturity`.

**A redemption fails for lack of balance.** The mock's yield is unbacked, so the
vault needs its strategy reserve. Top it up:

```bash
stellar contract invoke --id "$ASSET" --source-account strata-deployer \
  --network testnet -- transfer --from strata-deployer --to "$VAULT" \
  --amount 500000000
```

`run-epoch-demo.sh` does this automatically and prints when it does.

**The contracts are gone.** Testnet was reset. Redeploy; see the note at the top
of this document.