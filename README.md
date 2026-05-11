# Payments Engine

A streaming CSV payments processor written in Rust. Reads a transaction ledger, applies deposit, withdrawal, dispute, resolve, and chargeback operations, and outputs final account states to stdout.

## How to Run

```bash
# Run against a CSV file
cargo run -- transactions.csv

# Redirect output to a file
cargo run -- transactions.csv > accounts.csv

# Run tests
cargo test
```

## Project Structure

```
src/
├── main.rs       # CLI entry point — reads args, wires engine to stdout
├── engine.rs     # PaymentsEngine — all transaction logic and CSV I/O
├── model.rs      # Types: TransactionRecord, StoredTx, Account, AccountRecord
└── error.rs      # AppError wrapping std::io::Error and csv::Error
```

## Architecture Decisions

### Streaming over batch loading
The CSV reader processes one row at a time via `rdr.deserialize()` — the entire file is never held in memory. This keeps memory usage flat regardless of file size.

### Two HashMaps and a HashSet as the sole state
`PaymentsEngine` holds:
- `accounts: HashMap<u16, Account>` — live balance per client
- `transactions: HashMap<u32, StoredTx>` — deposits eligible for future dispute
- `finalised_txs: HashSet<u32>` — tx IDs that have been resolved or chargebacked

When a transaction is resolved or chargebacked it is removed from `transactions` and its ID is moved into `finalised_txs`. This prevents tx ID reuse: a deposit or withdrawal whose `tx` appears in either map is silently rejected. Memory is bounded to open (disputable) transactions plus the set of finalised IDs, not total transaction count.

### `rust_decimal` for monetary arithmetic
`f32`/`f64` cannot represent many decimal fractions exactly. `rust_decimal` uses a base-10 integer representation, giving exact arithmetic up to 28 significant digits. All amounts are stored and computed as `Decimal`, and formatted to 4 decimal places on output.

### Invalid rows are skipped, not fatal
Malformed CSV rows (bad type, missing field, non-numeric amount) are printed to stderr and skipped. The engine continues processing the remainder of the file. This matches real-world payment processing where one corrupt record should not abort the entire batch.

### Output is sorted by client ID
`write_accounts` collects all accounts, sorts by `client`, then serialises. This makes output deterministic regardless of HashMap iteration order.

## Assumptions

**Transaction ID uniqueness**
- `tx` IDs are globally unique across all transaction types. A duplicate `tx` ID on a deposit or withdrawal is silently rejected.
- Dispute, resolve, and chargeback reference an existing `tx` ID; they do not carry their own IDs.

**Disputes apply only to deposits**
- Only deposits are eligible for dispute. The engine checks `tx_type == Deposit` before accepting a dispute; disputes referencing a withdrawal tx ID are silently ignored.
- A dispute against an unknown or already-resolved/chargebacked `tx` is also ignored.

**Amounts are always positive**
- Deposits and withdrawals with `amount <= 0` are rejected. Dispute, resolve, and chargeback carry no amount field and use the stored value.

**`amount` field is optional in the CSV**
- Dispute, resolve, and chargeback rows may omit the amount column entirely. The CSV reader is configured with `flexible(true)` and `trim(All)` to handle missing or whitespace-padded fields.

**Account is created on first deposit or withdrawal**
- No pre-registration step. An account comes into existence the first time a valid deposit or withdrawal names it.

**Locked accounts reject all new deposits and withdrawals**
- A chargeback locks the account permanently for the lifetime of the process. Disputes, resolves, and chargebacks on pre-existing stored transactions are still processed (no locked check in those paths).

**Single-file, single-threaded processing**
- The current design processes one file sequentially. If multiple input files are guaranteed to carry disjoint client IDs and globally unique tx IDs, they can be processed in parallel (one `PaymentsEngine` per file, merge `accounts` maps at the end). See comments in `engine.rs` for the parallel design sketch.

## Known Limitations

- **Disputes are restricted to deposits** — the engine checks `tx_type == Deposit` before accepting a dispute, so withdrawal disputes are silently ignored.
- **`account.held` has no underflow guard** in `chargeback` or `resolve`. Under normal operation this cannot go negative, but it would if disputes are accepted on locked accounts (since `dispute` has no locked check).
- **No persistence** — state lives in memory for the duration of the process only.

## Dependencies

| Crate | Purpose |
|---|---|
| `csv` | Streaming CSV reader/writer with flexible field handling |
| `rust_decimal` | Exact base-10 decimal arithmetic for monetary values |
| `serde` | Derive-based serialisation/deserialisation |
| `thiserror` | Ergonomic error type definition |
