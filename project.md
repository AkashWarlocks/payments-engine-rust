# Payments Engine — Specification & Implementation Guide

> This file is intended to be read by Claude Code as context for implementing the payments engine.
> Place this file in the root of the `payments-engine` crate directory.

---

## Project Overview

Build a streaming CSV payments engine in Rust that:
- Reads transactions from a CSV file (first CLI argument)
- Processes deposits, withdrawals, disputes, resolves, and chargebacks
- Outputs final client account states as CSV to stdout

```bash
cargo run -- transactions.csv > accounts.csv
```

---

## File Structure

```
payments-engine/
├── Cargo.toml
├── PAYMENTS_ENGINE.md        ← this file
├── transactions.csv          ← sample input
└── src/
    ├── main.rs               ← CLI entry point only
    ├── model.rs              ← all data types
    ├── engine.rs             ← all business logic
    └── error.rs              ← custom error types
```

---

## Cargo.toml Dependencies

```toml
[dependencies]
csv = "1.3"
serde = { version = "1.0", features = ["derive"] }
rust_decimal = { version = "1.36", features = ["serde-with-str"] }
thiserror = "1.0"
```

| Crate          | Purpose                                                        |
| -------------- | -------------------------------------------------------------- |
| `csv`          | Stream rows one at a time — never load entire file into memory |
| `serde`        | Deserialize input rows and serialize output rows automatically |
| `rust_decimal` | Exact 4dp arithmetic — never use f64 for money                 |
| `thiserror`    | Clean error types with `#[from]` auto-conversion               |

---

## Data Types (model.rs)

### Input Row
```rust
#[derive(Debug, Deserialize)]
pub struct TransactionRecord {
    #[serde(rename = "type")]
    pub tx_type: TxType,
    pub client: u16,
    pub tx: u32,
    pub amount: Option<Decimal>, // Option — dispute/resolve/chargeback have no amount
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum TxType {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}
```

### Stored Transaction (in-memory)
```rust
pub struct StoredTx {
    pub client: u16,     // owner — for cross-client dispute guard
    pub amount: Decimal, // needed because dispute rows carry no amount
    pub disputed: bool,  // state machine flag
}
```

### Client Account (in-memory)
```rust
#[derive(Default)]
pub struct Account {
    pub available: Decimal,
    pub held: Decimal,
    pub locked: bool,
}

impl Account {
    pub fn total(&self) -> Decimal {
        self.available + self.held
    }
}
```

### Output Row
```rust
#[derive(Serialize)]
pub struct AccountRecord {
    pub client: u16,
    pub available: Decimal,
    pub held: Decimal,
    pub total: Decimal,
    pub locked: bool,
}
```

---

## Engine Struct (engine.rs)

```rust
pub struct PaymentsEngine {
    accounts: HashMap<u16, Account>,       // keyed by client u16
    transactions: HashMap<u32, StoredTx>,  // keyed by tx u32
}
```

Two HashMaps — that is all the state needed.

---

## Transaction Rules

### Deposit
- Validate amount is `Some(a)` and `a > 0`
- Reject duplicate tx IDs (already in `transactions` map)
- Skip if account is locked
- `account.available += amount`
- Insert into `transactions` map

### Withdrawal
- Validate amount is `Some(a)` and `a > 0`
- Reject duplicate tx IDs
- Skip if account is locked
- Skip if `account.available < amount` (insufficient funds — fail silently)
- `account.available -= amount`
- Insert into `transactions` map

### Dispute
- Look up tx in `transactions` — if not found, ignore (partner error)
- Guard: `tx.client == record.client` — ignore if mismatch
- Guard: `tx.disputed == false` — ignore if already disputed
- Set `tx.disputed = true`
- `account.available -= amount`
- `account.held += amount`
- Total stays the same

### Resolve
- Look up tx in `transactions` — if not found, ignore
- Guard: `tx.client == record.client`
- Guard: `tx.disputed == true` — ignore if not under dispute
- Set `tx.disputed = false`
- `account.held -= amount`
- `account.available += amount`
- Total stays the same

### Chargeback
- Look up tx in `transactions` — if not found, ignore
- Guard: `tx.client == record.client`
- Guard: `tx.disputed == true` — ignore if not under dispute
- Set `tx.disputed = false`
- `account.held -= amount`
- Total decreases — funds are reversed
- `account.locked = true` — account frozen permanently

---

## State Machine for a Transaction

```
[deposit stored]
      |
      | dispute
      v
  [disputed]
      |         \
   resolve     chargeback
      |               \
      v                v
[undisputed]       [gone, account locked]
```

---

## Streaming — Critical Rules

```rust
// CORRECT — lazy iterator, one row at a time
for result in rdr.deserialize::<TransactionRecord>() {
    match result {
        Ok(record) => engine.apply(record),
        Err(e) => eprintln!("Skipping bad row: {e}"), // never panic
    }
}

// WRONG — loads everything into memory
let rows: Vec<TransactionRecord> = rdr.deserialize().collect()?;
```

CSV Reader must be configured with:
```rust
ReaderBuilder::new()
    .trim(Trim::All)    // handles spaces around values
    .flexible(true)     // dispute rows have no amount column
    .from_reader(file)
```

---

## Output Rules

- Round all amounts to 4 decimal places using `.round_dp(4)`
- Sort output by client ID for deterministic results
- Write to stdout — caller redirects to file
- Use `csv::WriterBuilder` with serde serialize

---

## Edge Cases to Handle

| Case                                                   | Behaviour                          |
| ------------------------------------------------------ | ---------------------------------- |
| Dispute on non-existent tx                             | Ignore silently                    |
| Dispute on already-disputed tx                         | Ignore silently                    |
| Resolve on non-disputed tx                             | Ignore silently                    |
| Chargeback on non-disputed tx                          | Ignore silently                    |
| Cross-client dispute (client A disputes client B's tx) | Ignore silently                    |
| Withdrawal with insufficient funds                     | Ignore silently, no state change   |
| Deposit/withdrawal on locked account                   | Ignore silently                    |
| Duplicate tx ID                                        | Ignore silently                    |
| Malformed CSV row                                      | Log to stderr, continue processing |
| Negative or zero amount                                | Ignore silently                    |

---

## Assumptions (document in README)

1. Only deposits can be disputed — disputing a withdrawal would add money back, which is not standard banking behaviour and not described in the spec
2. Transactions are processed strictly in file order — chronological by spec
3. A locked account ignores all further deposits and withdrawals
4. Disputes can only be raised by the client who owns the transaction
5. Duplicate transaction IDs are silently rejected
6. After chargeback, the transaction is removed from disputed state (it is finalised)

---

## Sample Input (transactions.csv)

```
type,client,tx,amount
deposit,1,1,100.0
deposit,2,2,200.0
deposit,3,3,300.0
withdrawal,1,4,20.0
withdrawal,2,5,50.0
withdrawal,3,6,100.0
```

## Expected Output

```
client,available,held,total,locked
1,80.0000,0.0000,80.0000,false
2,150.0000,0.0000,150.0000,false
3,200.0000,0.0000,200.0000,false
```

---

## Dispute Scenario Example

```
type,client,tx,amount
deposit,1,1,100.0
dispute,1,1,
chargeback,1,1,
```

Expected output — client 1 has 0 funds and is locked:
```
client,available,held,total,locked
1,0.0000,0.0000,0.0000,true
```

---

## Memory Considerations

- `accounts` map is bounded by `u16::MAX` = 65,535 entries (~3 MB max)
- `transactions` map grows with deposits/withdrawals
- Remove transactions after chargeback or resolve to free memory
- Never collect the CSV iterator — always stream row by row

---

## Error Type (error.rs)

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
}
```

---

## main.rs Pattern

```rust
fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <transactions.csv>", args[0]);
        process::exit(1);
    }

    let mut engine = PaymentsEngine::new();
    if let Err(e) = engine.process_file(&args[1]) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
    if let Err(e) = engine.write_accounts(io::stdout()) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
```

---

## Unit Tests to Write

Cover these cases in `engine.rs` under `#[cfg(test)]`:

- `deposit_increases_available_and_total`
- `withdrawal_decreases_available`
- `withdrawal_fails_if_insufficient_funds`
- `withdrawal_fails_on_locked_account`
- `dispute_moves_funds_to_held`
- `resolve_releases_held_funds`
- `chargeback_removes_held_and_locks_account`
- `duplicate_tx_id_is_rejected`
- `dispute_on_unknown_tx_is_ignored`
- `chargeback_without_prior_dispute_is_ignored`
- `cross_client_dispute_is_ignored`

---

## Claude Code Prompts to Use

Use these prompts in sequence with Claude Code CLI:

```bash
# 1. Scaffold
claude "Using PAYMENTS_ENGINE.md as the spec, scaffold the full 
payments-engine Rust project with all four files: main.rs, 
model.rs, engine.rs, error.rs. Do not implement logic yet, 
just the types and empty function stubs."

# 2. Implement engine
claude "Implement all transaction methods in engine.rs following 
the rules in PAYMENTS_ENGINE.md exactly. Pay special attention 
to the edge cases table."

# 3. Add tests
claude "Add all unit tests listed in PAYMENTS_ENGINE.md to 
engine.rs. Use a helper that builds an engine from a CSV string."

# 4. Run and fix
claude "Run cargo test and fix any failures."

# 5. Verify with sample
claude "Run the engine against transactions.csv and verify the 
output matches the expected output in PAYMENTS_ENGINE.md."

# 6. Write README
claude "Write a README.md covering: how to run, architecture 
decisions, assumptions made, and how AI was used in development."
```