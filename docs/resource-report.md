# Epoch manager resource budget report

**Strata is unaudited and testnet-only.** This report measures the per-transaction CPU instructions, memory allocation, ledger read/write footprint, storage write bytes, and contract event bytes of every entrypoint on [`contracts/epoch-manager`](../contracts/epoch-manager/src/lib.rs) across `N = 1`, `10`, and `100` depositor positions, closing issue #7.

---

## 1. Environment and reproduction

| Field | Value |
| --- | --- |
| Date measured | `2026-11-08` |
| Pinned `soroban-sdk` | `27.0.6` (`soroban-env-host 27.0.1`) |
| Rust toolchain | `1.98.0` (`overflow-checks = true` in all profiles) |
| Metering API | `Env::cost_estimate().resources()` (`InvocationResources`) and `Env::cost_estimate().fee()` (`FeeEstimate`) |
| Test harness | [`tests/src/resource_budget.rs`](../tests/src/resource_budget.rs) |

Reproduce every table and assertion below with:

```bash
cargo test -p strata-tests resource_budget -- --nocapture
```

> **Note on native vs. Wasm metering (`soroban-sdk` 27.0.6):** `EpochManagerClient` in `strata-tests` invokes the contract registered via `env.register(...)` in `soroban-env-host`. As documented on `soroban_sdk::testutils::cost_estimate::CostEstimate::resources()`, ledger footprint counts (`memory_read_entries`, `disk_read_entries`, `write_entries`), serialized XDR byte sizes (`write_bytes`, `disk_read_bytes`, `contract_events_size_bytes`), and host function/storage metering are exact, whereas Wasm VM instantiation and bytecode fuel adds a fixed per-contract overhead on a live network.

---

## 2. Stellar network per-transaction resource limits

Checked on **2026-11-08** against the official Stellar documentation ([Resource Limits & Fees](https://developers.stellar.org/docs/networks/resource-limits-fees)), [Stellar Lab Network Limits](https://lab.stellar.org/network-limits), and the mainnet limits enforced automatically by `Env::default()` in `soroban-sdk 27.0.6` (`NetworkInvocationResourceLimits::mainnet()` in `soroban-sdk-27.0.6/src/testutils/cost_estimate.rs`):

| Resource dimension | Stellar Mainnet per-tx limit | Enforced by `Env::default()` (`soroban-sdk 27.0.6`) |
| --- | ---: | --- |
| CPU instructions (`instructions`) | `400,000,000` (400M) | Yes (`400_000_000`) |
| Memory bytes (`mem_bytes`) | `41,943,040` (40 MiB) | Yes (`41_943_040`) |
| Total ledger footprint (`ledger_entries` = read + write) | `400` entries | Yes (`400`) |
| Disk read entries (`disk_read_entries`) | `200` entries | Yes (`200`) |
| Write entries (`write_entries`) | `200` entries | Yes (`200`) |
| Disk read bytes (`disk_read_bytes`) | `200,000` B (~195.3 KiB) | Yes (`200_000`) |
| Ledger write bytes (`write_bytes`) | `132,096` B (129 KiB) | Yes (`132_096`) |
| Contract events + return value size (`contract_events_size_bytes`) | `16,384` B (16 KiB) | Yes (`16_384`) |
| Max contract data key size | `250` B | Yes (`250`) |
| Max contract data entry size | `65,536` B (64 KiB) | Yes (`65_536`) |
| Max contract code (Wasm) entry size | `131,072` B (128 KiB) | Yes (`131_072`) |

---

## 3. Measurements: `EpochManager` + `MockVault` (`N = 1`, `10`, `100` positions)

For `N = 1`, the epoch holds 1 Junior position (`1,000,000` stroops); `deposit (Senior)` at `N = 1` is measured on an isolated 1-Junior epoch so the `N = 1` lifecycle retains strictly 1 position through `settle`, `claim`, and `close_epoch`. For `N = 10` and `N = 100`, the epoch holds `N / 2` Junior positions and `N / 2` Senior positions (`1,000,000` stroops each).

| Entrypoint | `N` | CPU instructions | % of 400M | Memory (bytes) | % of 40 MiB | Mem Read Entries | Disk Read Entries | Write Entries | Total Footprint ($\le 400$) | Write Bytes ($\le 132,096$) | Event Bytes ($\le 16,384$) | Rent Bumps | Est. Fee (stroops) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `__constructor` | 1 | 90,365 | 0.0226% | 14,321 | 0.0341% | 5 | 0 | 2 | 7 | 368 | 228 | 2 | 10,203,995 |
| `__constructor` | 10 | 90,365 | 0.0226% | 14,321 | 0.0341% | 5 | 0 | 2 | 7 | 368 | 228 | 2 | 10,203,995 |
| `__constructor` | 100 | 90,365 | 0.0226% | 14,321 | 0.0341% | 5 | 0 | 2 | 7 | 368 | 228 | 2 | 10,203,995 |
| `create_epoch` | 1 | 101,846 | 0.0255% | 17,254 | 0.0411% | 5 | 0 | 2 | 7 | 780 | 244 | 1 | 29,521,623 |
| `create_epoch` | 10 | 101,846 | 0.0255% | 17,254 | 0.0411% | 5 | 0 | 2 | 7 | 780 | 244 | 1 | 29,521,623 |
| `create_epoch` | 100 | 101,846 | 0.0255% | 17,254 | 0.0411% | 5 | 0 | 2 | 7 | 780 | 244 | 1 | 29,521,623 |
| `deposit (Junior)` | 1 | 709,809 | 0.1775% | 100,419 | 0.2394% | 16 | 1 | 9 | 26 | 1,988 | 900 | 3 | 10,008,243 |
| `deposit (Junior)` | 10 | 821,906 | 0.2055% | 135,053 | 0.3220% | 16 | 0 | 9 | 25 | 1,988 | 900 | 1 | 8,875,791 |
| `deposit (Junior)` | 100 | 1,594,754 | 0.3987% | 520,613 | 1.2412% | 16 | 0 | 9 | 25 | 1,988 | 900 | 1 | 8,876,332 |
| `deposit (Senior)` | 1 | 756,101 | 0.1890% | 102,989 | 0.2455% | 16 | 0 | 9 | 25 | 1,988 | 900 | 1 | 8,875,745 |
| `deposit (Senior)` | 10 | 884,031 | 0.2210% | 153,413 | 0.3658% | 16 | 0 | 9 | 25 | 1,988 | 900 | 1 | 8,875,834 |
| `deposit (Senior)` | 100 | 1,930,256 | 0.4826% | 704,213 | 1.6790% | 16 | 0 | 9 | 25 | 1,988 | 900 | 1 | 8,876,567 |
| `admin` | 1 | 24,368 | 0.0061% | 3,759 | 0.0090% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 18 |
| `admin` | 10 | 25,302 | 0.0063% | 6,819 | 0.0163% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 18 |
| `admin` | 100 | 36,552 | 0.0091% | 37,419 | 0.0892% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 26 |
| `vault` | 1 | 24,368 | 0.0061% | 3,759 | 0.0090% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 18 |
| `vault` | 10 | 25,302 | 0.0063% | 6,819 | 0.0163% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 18 |
| `vault` | 100 | 36,552 | 0.0091% | 37,419 | 0.0892% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 26 |
| `asset` | 1 | 24,368 | 0.0061% | 3,759 | 0.0090% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 18 |
| `asset` | 10 | 25,302 | 0.0063% | 6,819 | 0.0163% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 18 |
| `asset` | 100 | 36,552 | 0.0091% | 37,419 | 0.0892% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 26 |
| `current_epoch` | 1 | 67,062 | 0.0168% | 7,681 | 0.0183% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 47 |
| `current_epoch` | 10 | 66,037 | 0.0165% | 10,741 | 0.0256% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 47 |
| `current_epoch` | 100 | 76,462 | 0.0191% | 41,341 | 0.0986% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 54 |
| `position_of (Open)` | 1 | 115,221 | 0.0288% | 12,996 | 0.0310% | 7 | 0 | 0 | 7 | 0 | 0 | 0 | 81 |
| `position_of (Open)` | 10 | 120,578 | 0.0301% | 19,116 | 0.0456% | 7 | 0 | 0 | 7 | 0 | 0 | 0 | 85 |
| `position_of (Open)` | 100 | 146,789 | 0.0367% | 80,316 | 0.1915% | 7 | 0 | 0 | 7 | 0 | 0 | 0 | 103 |
| `senior_room` | 1 | 48,191 | 0.0120% | 5,676 | 0.0135% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 34 |
| `senior_room` | 10 | 47,166 | 0.0118% | 8,736 | 0.0208% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 34 |
| `senior_room` | 100 | 57,591 | 0.0144% | 39,336 | 0.0938% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 41 |
| `seconds_to_maturity` | 1 | 48,193 | 0.0120% | 5,676 | 0.0135% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 34 |
| `seconds_to_maturity` | 10 | 47,168 | 0.0118% | 8,736 | 0.0208% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 34 |
| `seconds_to_maturity` | 100 | 57,593 | 0.0144% | 39,336 | 0.0938% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 41 |
| `project` | 1 | 52,381 | 0.0131% | 6,176 | 0.0147% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 37 |
| `project` | 10 | 51,356 | 0.0128% | 9,236 | 0.0220% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 36 |
| `project` | 100 | 61,781 | 0.0154% | 39,836 | 0.0950% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 44 |
| `vault_shares` | 1 | 48,191 | 0.0120% | 5,676 | 0.0135% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 34 |
| `vault_shares` | 10 | 47,166 | 0.0118% | 8,736 | 0.0208% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 34 |
| `vault_shares` | 100 | 57,591 | 0.0144% | 39,336 | 0.0938% | 2 | 0 | 0 | 2 | 0 | 0 | 0 | 41 |
| `settle` | 1 | 657,185 | 0.1643% | 82,799 | 0.1974% | 13 | 0 | 7 | 20 | 1,628 | 744 | 1 | 190,790 |
| `settle` | 10 | 737,818 | 0.1845% | 122,579 | 0.2923% | 13 | 0 | 7 | 20 | 1,628 | 744 | 1 | 190,846 |
| `settle` | 100 | 1,367,805 | 0.3420% | 520,379 | 1.2407% | 13 | 0 | 7 | 20 | 1,628 | 744 | 1 | 191,287 |
| `position_of (Settled)` | 1 | 62,739 | 0.0157% | 7,179 | 0.0171% | 3 | 0 | 0 | 3 | 0 | 0 | 0 | 44 |
| `position_of (Settled)` | 10 | 66,722 | 0.0167% | 10,239 | 0.0244% | 3 | 0 | 0 | 3 | 0 | 0 | 0 | 47 |
| `position_of (Settled)` | 100 | 81,935 | 0.0205% | 40,839 | 0.0974% | 3 | 0 | 0 | 3 | 0 | 0 | 0 | 58 |
| `claim (first in tranche)` | 1 | 364,281 | 0.0911% | 51,467 | 0.1227% | 9 | 0 | 5 | 14 | 1,404 | 424 | 0 | 2,218,050 |
| `claim (first in tranche)` | 10 | 427,774 | 0.1069% | 75,947 | 0.1811% | 9 | 0 | 5 | 14 | 1,404 | 424 | 0 | 2,218,095 |
| `claim (first in tranche)` | 100 | 883,677 | 0.2209% | 320,747 | 0.7647% | 9 | 0 | 5 | 14 | 1,404 | 424 | 0 | 2,218,414 |
| `claim (last in tranche)` | 1 | 364,281 | 0.0911% | 51,467 | 0.1227% | 9 | 0 | 5 | 14 | 1,404 | 424 | 0 | 2,218,050 |
| `claim (last in tranche)` | 10 | 462,770 | 0.1157% | 78,123 | 0.1863% | 9 | 0 | 5 | 14 | 1,404 | 424 | 0 | 2,218,119 |
| `claim (last in tranche)` | 100 | 922,443 | 0.2306% | 347,403 | 0.8283% | 9 | 0 | 5 | 14 | 1,404 | 424 | 0 | 2,218,441 |
| `close_epoch` | 1 | 113,109 | 0.0283% | 16,540 | 0.0394% | 2 | 0 | 1 | 3 | 712 | 148 | 0 | 5,475 |
| `close_epoch` | 10 | 119,820 | 0.0300% | 23,884 | 0.0569% | 2 | 0 | 1 | 3 | 712 | 148 | 0 | 5,479 |
| `close_epoch` | 100 | 217,053 | 0.0543% | 97,324 | 0.2320% | 2 | 0 | 1 | 3 | 712 | 148 | 0 | 5,547 |

---

## 4. Measurements: `EpochManager` + `BlendAdapter` (`N = 1`, `10`, `100` positions)

Because `deposit`, `settle`, `__constructor`, and `position_of (Open)` cross-call the underlying vault, wrapping [`BlendAdapter`](../contracts/blend-adapter/src/lib.rs) (which in turn calls the Blend lending pool) increases the call depth by one contract frame (`EpochManager` $\to$ `BlendAdapter` $\to$ `BlendPool`). All other entrypoints (`create_epoch`, `claim`, `close_epoch`, and the non-vault views) do not call the vault and have identical footprints to Section 3.

| Entrypoint | `N` | CPU instructions | % of 400M | Memory (bytes) | % of 40 MiB | Mem Read Entries | Disk Read Entries | Write Entries | Total Footprint ($\le 400$) | Write Bytes ($\le 132,096$) | Event Bytes ($\le 16,384$) | Rent Bumps | Est. Fee (stroops) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `__constructor (BlendAdapter)` | 1 | 99,490 | 0.0249% | 18,129 | 0.0432% | 5 | 0 | 2 | 7 | 368 | 228 | 2 | 10,204,001 |
| `__constructor (BlendAdapter)` | 10 | 99,490 | 0.0249% | 18,129 | 0.0432% | 5 | 0 | 2 | 7 | 368 | 228 | 2 | 10,204,001 |
| `__constructor (BlendAdapter)` | 100 | 99,490 | 0.0249% | 18,129 | 0.0432% | 5 | 0 | 2 | 7 | 368 | 228 | 2 | 10,204,001 |
| `deposit (Junior, BlendAdapter)` | 1 | 1,434,892 | 0.3587% | 206,748 | 0.4929% | 29 | 1 | 13 | 43 | 2,564 | 1,172 | 6 | 20,858,478 |
| `deposit (Junior, BlendAdapter)` | 10 | 1,608,362 | 0.4021% | 247,216 | 0.5894% | 29 | 0 | 13 | 42 | 2,564 | 1,172 | 1 | 8,894,413 |
| `deposit (Junior, BlendAdapter)` | 100 | 2,691,397 | 0.6728% | 825,556 | 1.9683% | 29 | 0 | 13 | 42 | 2,564 | 1,172 | 1 | 8,895,171 |
| `deposit (Senior, BlendAdapter)` | 10 | 1,668,025 | 0.4170% | 274,756 | 0.6551% | 29 | 0 | 13 | 42 | 2,564 | 1,172 | 1 | 8,894,455 |
| `deposit (Senior, BlendAdapter)` | 100 | 3,121,548 | 0.7804% | 1,100,956 | 2.6249% | 29 | 0 | 13 | 42 | 2,564 | 1,172 | 1 | 8,895,473 |
| `position_of (Open, BlendAdapter)` | 1 | 306,481 | 0.0766% | 34,711 | 0.0828% | 18 | 0 | 0 | 18 | 0 | 0 | 0 | 215 |
| `position_of (Open, BlendAdapter)` | 10 | 317,246 | 0.0793% | 46,951 | 0.1119% | 18 | 0 | 0 | 18 | 0 | 0 | 0 | 223 |
| `position_of (Open, BlendAdapter)` | 100 | 377,993 | 0.0945% | 169,351 | 0.4038% | 18 | 0 | 0 | 18 | 0 | 0 | 0 | 265 |
| `settle (BlendAdapter)` | 1 | 1,423,101 | 0.3558% | 186,546 | 0.4448% | 26 | 0 | 11 | 37 | 2,216 | 1,016 | 1 | 209,408 |
| `settle (BlendAdapter)` | 10 | 1,586,597 | 0.3966% | 256,926 | 0.6126% | 26 | 0 | 11 | 37 | 2,216 | 1,016 | 1 | 209,522 |
| `settle (BlendAdapter)` | 100 | 2,566,739 | 0.6417% | 960,726 | 2.2905% | 26 | 0 | 11 | 37 | 2,216 | 1,016 | 1 | 210,208 |

---

## 5. Comparison with network limits: which entrypoints come closest?

No entrypoint comes close to exhausting any Stellar Mainnet per-transaction resource limit. Ranking the dimensions from highest relative utilisation to lowest:

1. **Total ledger footprint (`ledger_entries <= 400`) and write entries (`write_entries <= 200`):**
   - `deposit` via `BlendAdapter` is the widest transaction in the system: **42 / 400 total footprint entries (10.50%)** (`43 / 400 = 10.75%` on the very first deposit into an empty pool) and **13 / 200 write entries (6.50%)**, because a single deposit touches `EpochManager` (`Config`, `Epoch`, `Position`), the SEP-41 token (`Balance(user)`, `Balance(manager)`, `Balance(adapter)`, `Balance(pool)`), `BlendAdapter` (`Config`, `Position`), and the Blend pool's reserve and user position entries.
   - `settle` via `BlendAdapter` is second: **37 / 400 total footprint entries (9.25%)** and **11 / 200 write entries (5.50%)**.
   - With `MockVault`, `deposit` uses **25 / 400 footprint entries (6.25%)** (`9 / 200 = 4.50%` write entries) and `settle` uses **20 / 400 footprint entries (5.00%)** (`7 / 200 = 3.50%` write entries).
2. **Contract event bytes (`contract_events_size_bytes <= 16,384 B`):**
   - `deposit (BlendAdapter)` emits **1,172 / 16,384 B (7.15%)** across the token transfers, pool supply, adapter deposit, and manager `DepositEvent`.
   - `settle (BlendAdapter)` emits **1,016 / 16,384 B (6.20%)**.
   - `deposit (MockVault)` emits **900 / 16,384 B (5.49%)**, and `settle (MockVault)` emits **744 / 16,384 B (4.54%)**.
3. **Ledger write bytes (`write_bytes <= 132,096 B`):**
   - Peaks on `deposit (BlendAdapter)` at **2,564 / 132,096 B (1.94%)** and `settle (BlendAdapter)` at **2,216 / 132,096 B (1.68%)**.
   - Within `EpochManager` itself, the largest single entry written is `DataKey::Epoch` (`712 B` XDR in `close_epoch`), which is **1.09%** of the `65,536 B` single-entry limit.
4. **Memory (`mem_bytes <= 41,943,040 B`) and CPU instructions (`instructions <= 400,000,000`):**
   - At `N = 1` (without test-harness map bloat), peak memory is **206,748 B (0.4929% of 40 MiB)** and peak CPU is **1,434,892 instructions (0.3587% of 400M)** on `deposit (BlendAdapter)`.
   - Even at `N = 100` (including the in-memory test-harness `Storage.map` cloning overhead explained in Section 6), peak memory is **1,100,956 B (2.6249% of 40 MiB)** and peak CPU is **3,121,548 instructions (0.7804% of 400M)**.

---

## 6. Scaling with depositor count (`N = 1`, `10`, `100`)

### A. On-chain per-transaction cost is strictly $O(1)$

In [`contracts/epoch-manager/src/lib.rs`](../contracts/epoch-manager/src/lib.rs), `EpochManager` never stores a `Vec` or `Map` of depositors and never loops over positions:

- `DataKey::Epoch` stores aggregate tranche principals (`senior_total`, `junior_total`), aggregate unclaimed principals (`senior_unclaimed`, `junior_unclaimed`), aggregate vault shares (`vault_shares`), and aggregate payouts (`senior_payout`, `junior_payout`, `senior_paid`, `junior_paid`) as fixed-width scalars (`u64`, `u32`, `i128`).
- Each depositor's principal lives in its own isolated persistent entry, `DataKey::Position(Address, Tranche)`.
- `settle` redeems `epoch.vault_shares` in a single vault call and runs `strata_waterfall::settle` once on the aggregate totals (`13` memory reads, `7` writes, `1,628` write bytes for `MockVault`; `26` memory reads, `11` writes, `2,216` write bytes for `BlendAdapter` at `N = 1`, `10`, and `100`).
- `claim` reads and clears only the caller's own `DataKey::Position(claimant, tranche)` entry and updates the running counters on `DataKey::Epoch` (`9` memory reads, `5` writes, `1,404` write bytes at `N = 1`, `10`, and `100`).
- `close_epoch` checks `epoch.senior_unclaimed > 0 || epoch.junior_unclaimed > 0` in $O(1)$ (`2` memory reads, `1` write, `712` write bytes at `N = 1`, `10`, and `100`).

### B. Why `instructions` and `mem_bytes` grow slightly with `N` in `soroban-sdk` tests

In Tables 3 and 4, `memory_read_entries`, `disk_read_entries`, `write_entries`, `write_bytes`, and `contract_events_size_bytes` are constant from `N = 10` to `N = 100`, yet `instructions` and `mem_bytes` increase slightly with `N` — even on `admin()`, which only reads `DataKey::Config` and never touches `Epoch` or `Position`:

- `admin()` at `N = 1`: `3,759 B`
- `admin()` at `N = 10` (`+9` depositors): `6,819 B` (`+3,060 B = 9 * 340 B`)
- `admin()` at `N = 100` (`+99` depositors): `37,419 B` (`+33,660 B = 99 * 340 B`)

`host_storage_map_cloning_explains_in_memory_instruction_slope_not_contract_loops` in [`tests/src/resource_budget.rs`](../tests/src/resource_budget.rs) proves why:

1. On a **live Stellar validator**, the host constructs `Storage.map` for a transaction exclusively from that transaction's declared footprint (`2` entries for `admin()`, `20` entries for `settle()`). Unrelated `DataKey::Position` entries are not loaded into host memory.
2. In **`soroban-env-host 27.0.1`'s in-memory test `Env`**, `Storage.map` (`MeteredOrdMap`) is never pruned between calls (`soroban-env-host/src/host/invocation_metering.rs:735`). Each depositor in the test fixture adds 3 entries to `Storage.map` (`Balance(user)`, auth `Nonce(user)`, and `DataKey::Position(user, tranche)`). Whenever `InvocationMeter` or a sub-contract call snapshots or mutates `Storage.map`, `MeteredOrdMap` clones the underlying vector of all live entries in `Env` and charges `ContractCostType::MemAlloc` / `MemCpy` (`340 B` per depositor per sub-call frame: `1 * 340 B` for 1-frame views like `admin()`, `2 * 340 = 680 B` for 2-frame `position_of (Open)` on `MockVault`, and `4 * 340 = 1,360 B` for 4-frame `position_of (Open)` on `BlendAdapter`).

Because no entrypoint's on-chain cost scales with depositor count, **no per-entrypoint scaling issue needs to be filed**.

### C. Lifecycle-level note: `N` separate `claim` transactions precede `close_epoch`

While every individual transaction is $O(1)$, `close_epoch` requires `epoch.senior_unclaimed == 0 && epoch.junior_unclaimed == 0` (`Error::ClaimsOutstanding`), and `claim(env, claimant, tranche)` requires `claimant.require_auth()` (so a third party cannot batch-claim on behalf of passive depositors). Closing an epoch with `N` positions therefore requires `N` separate `claim` transactions across the epoch lifecycle, and a single depositor who never claims blocks `close_epoch` from opening a subsequent epoch.
