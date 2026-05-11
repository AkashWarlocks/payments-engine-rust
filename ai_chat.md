# AI Usage in This Project

This project was built as a Rust study exercise. Claude Code (claude-sonnet-4-6) was used as a pair-programming tool throughout — for code review, edge case analysis, and test strategy planning. This document summarises the key AI-assisted decisions and what was accepted or rejected.

---

## Code Review — Chargeback Edge Cases

**Prompt:** "Review my chargeback implementation for edge cases — specifically around disputed state checks and account locking."

### Findings

| Edge Case | Risk | Guarded? | Action Taken |
|---|---|---|---|
| Chargeback without prior dispute | Correct — `!stored.disputed` blocks it | Yes | No change needed |
| Double chargeback | Correct — tx removed after first | Yes | No change needed |
| Cross-client chargeback | Correct — `stored.client != client` blocks it | Yes | No change needed |
| `held` going negative | Silent decimal corruption in chargeback | No | **Fixed** — added `get_mut()` guard |
| `or_default()` creating ghost account | Negative held on fresh account | No | **Fixed** — replaced with `get_mut()` + early return |
| Dispute on locked account | Can manipulate held post-lock | No | **Accepted as known limitation** — see README |
| Dispute/chargeback on withdrawal tx | Double-penalises available | No | **Fixed** — store `TxType` in `StoredTx`, guard dispute to deposits only |

### Key fix: replacing `or_default()` with `get_mut()`

`accounts.entry(client).or_default()` in `resolve` and `chargeback` masked a logical impossibility — an account must already exist if a stored tx references it. Replaced with:

```rust
let Some(account) = self.accounts.get_mut(&client) else { return };
```

---

## Rust Best Practices Review

**Tool used:** `/rust-skills` (179-rule reference) and `/rust-best-practices` (Apollo GraphQL handbook)

### Findings reviewed

| Finding | Rule | Decision |
|---|---|---|
| `Vec::collect()` without `with_capacity()` | `mem-with-capacity` | **Rejected** — `HashMap::iter()` implements `ExactSizeIterator`; `.collect()` already pre-allocates. Not a real issue. |
| No `Default` impl on `PaymentsEngine` | `api-default-impl` | **Deferred** — only matters if published as a library. This is a binary. |
| `process_file` accepts `&str` not `impl AsRef<Path>` | `api-impl-asref` | **Deferred** — same reasoning; not a library API concern for this project. |

### Changes that were applied

- Added `///` doc comments on all public types in `model.rs` and `error.rs`
- Added inline `//` comments throughout `engine.rs` explaining non-obvious logic (why `trim(All)`, why only deposits are disputable, why `finalised_txs` prevents ID reuse)
- Import ordering: `std` → external crates → `crate::` (per Apollo Ch 1.7)

---

## Testing Strategy

**Prompt:** "Generate plans and steps for: 1. property testing using proptest, 2. cargo-fuzz, 3. invariant testing."

AI suggested implementing in order: **invariants first** (lowest effort, highest leverage), then proptest, then fuzzing. This ordering was adopted.

### Invariant tests

Added `assert_invariants()` as a `#[cfg(test)]`-only method on `PaymentsEngine`. Called automatically in `engine_from_csv` so every existing and future test validates state correctness after each scenario.

Invariants checked:
- `total() == available + held` for every account
- `held >= 0` for every account
- `finalised_txs ∩ transactions.keys() == ∅` (no tx ID in both maps simultaneously)

Note: `available >= 0` was intentionally **excluded** — a deposit that is disputed after a partial withdrawal can legally drive `available` negative. AI correctly flagged this as a valid state, not a bug.

### Property tests

Added 9 property tests using `proptest` in `engine::tests::property`. Key strategies:

```rust
prop_compose! {
    fn arb_amount()(n in 1u64..1_000_000) -> Decimal {
        Decimal::new(n as i64, 2)  // 0.01 .. 10000.00
    }
}
```

Properties tested: deposit round-trips, withdrawal never overdrafts, dispute preserves total, chargeback always locks, arbitrary op sequences never violate invariants.

### Fuzz targets

Two fuzz targets under `fuzz/fuzz_targets/`:

| Target | What it fuzzes |
|---|---|
| `fuzz_target_1` | Full CSV → engine pipeline — arbitrary bytes, engine must never panic |
| `fuzz_single_record` | CSV deserialiser in isolation — catches serde/csv parsing panics |

This required splitting the crate into a binary + library (`src/lib.rs`) so fuzz targets could import `PaymentsEngine`, and extracting `process_reader<R: Read>` alongside `process_file` for fuzz and test use.

---

## What Was Intentionally Not Fixed

- **`available >= 0` invariant** — legally negative in dispute-after-partial-withdrawal scenario
- **Dispute on locked accounts** — pre-existing stored transactions are still processed post-lock; documented in README as a known limitation
- **`Default` / `Debug` derive on `PaymentsEngine`** — not needed for a CLI binary
- **`process_file` using `impl AsRef<Path>`** — same reasoning
