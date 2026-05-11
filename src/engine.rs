use crate::error::Result;
use crate::model::{Account, AccountRecord, StoredTx, TransactionRecord, TxType};
use csv::ReaderBuilder;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::io::Write;

/// Processes payment transactions and maintains per-client account state.
///
/// Three collections form the complete state:
/// - `accounts`: live balance per client
/// - `transactions`: deposits eligible for future dispute (removed on finalisation)
/// - `finalised_txs`: tx IDs retired via resolve/chargeback, kept only to block ID reuse
pub struct PaymentsEngine {
    accounts: HashMap<u16, Account>,
    transactions: HashMap<u32, StoredTx>,
    finalised_txs: HashSet<u32>,
}

impl PaymentsEngine {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            transactions: HashMap::new(),
            finalised_txs: HashSet::new(),
        }
    }

    /// Reads and applies all transactions from a CSV file at `path`.
    ///
    /// Malformed rows are printed to stderr and skipped; processing continues.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or read.
    pub fn process_file(&mut self, path: &str) -> Result<()> {
        let file = std::fs::File::open(path)?;
        let mut rdr = ReaderBuilder::new()
            // trim(All): input files may have spaces around field values
            .trim(csv::Trim::All)
            // flexible(true): dispute/resolve/chargeback rows legally omit the amount column
            .flexible(true)
            .from_reader(file);

        for result in rdr.deserialize::<TransactionRecord>() {
            match result {
                Ok(record) => self.apply(record),
                Err(e) => eprintln!("Skipping bad row: {e}"),
            }
        }
        Ok(())
    }

    /// Serialises all account states as CSV to `writer`, sorted by client ID.
    ///
    /// # Errors
    /// Returns an error if serialisation or flushing fails.
    pub fn write_accounts<W: Write>(&self, writer: W) -> Result<()> {
        let mut wtr = csv::WriterBuilder::new().from_writer(writer);
        let mut records: Vec<AccountRecord> = self
            .accounts
            .iter()
            .map(|(&client, acc)| {
                let available = acc.available.round_dp(4);
                let held = acc.held.round_dp(4);
                AccountRecord {
                    client,
                    available: format!("{:.4}", available),
                    held: format!("{:.4}", held),
                    total: format!("{:.4}", acc.total().round_dp(4)),
                    locked: acc.locked,
                }
            })
            .collect();
        // HashMap iteration order is non-deterministic; sort for reproducible output
        records.sort_by_key(|r| r.client);
        for record in &records {
            wtr.serialize(record)?;
        }
        wtr.flush()?;
        Ok(())
    }

    fn apply(&mut self, record: TransactionRecord) {
        match record.tx_type {
            TxType::Deposit => self.deposit(record.client, record.tx, record.amount),
            TxType::Withdrawal => self.withdrawal(record.client, record.tx, record.amount),
            TxType::Dispute => self.dispute(record.client, record.tx),
            TxType::Resolve => self.resolve(record.client, record.tx),
            TxType::Chargeback => self.chargeback(record.client, record.tx),
        }
    }

    fn deposit(&mut self, client: u16, tx: u32, amount: Option<Decimal>) {
        let Some(amount) = amount else { return };
        if amount <= Decimal::ZERO {
            return;
        }
        // reject if tx ID was already used — finalised_txs catches IDs that were
        // removed from the live map after resolve/chargeback, preventing reuse
        if self.transactions.contains_key(&tx) || self.finalised_txs.contains(&tx) {
            return;
        }
        let account = self.accounts.entry(client).or_default();
        if account.locked {
            return;
        }
        account.available += amount;
        self.transactions.insert(
            tx,
            StoredTx {
                client,
                amount,
                disputed: false,
                tx_type: TxType::Deposit,
            },
        );
    }

    fn withdrawal(&mut self, client: u16, tx: u32, amount: Option<Decimal>) {
        let Some(amount) = amount else { return };
        if amount <= Decimal::ZERO {
            return;
        }
        // same dual-map check as deposit — prevents tx ID reuse after finalisation
        if self.transactions.contains_key(&tx) || self.finalised_txs.contains(&tx) {
            return;
        }
        let account = self.accounts.entry(client).or_default();
        if account.locked {
            return;
        }
        if account.available < amount {
            return;
        }
        account.available -= amount;
        self.transactions.insert(
            tx,
            StoredTx {
                client,
                amount,
                disputed: false,
                tx_type: TxType::Withdrawal,
            },
        );
    }

    fn dispute(&mut self, client: u16, tx: u32) {
        let Some(stored) = self.transactions.get_mut(&tx) else {
            return;
        };
        // only deposits are disputable — disputing a withdrawal would move funds
        // to held that were already removed from available, driving available negative
        if stored.client != client || stored.disputed || stored.tx_type != TxType::Deposit {
            return;
        }
        // no locked check: a chargeback only blocks new deposits/withdrawals;
        // disputes on pre-existing stored transactions remain valid after locking
        stored.disputed = true;
        let amount = stored.amount;
        let account = self.accounts.entry(client).or_default();
        account.available -= amount;
        account.held += amount;
    }

    fn resolve(&mut self, client: u16, tx: u32) {
        let Some(stored) = self.transactions.get_mut(&tx) else {
            return;
        };
        if stored.client != client || !stored.disputed {
            return;
        }
        let amount = stored.amount;
        // move to finalised_txs rather than just removing — prevents this tx ID
        // from being accepted again in a future deposit or withdrawal
        self.transactions.remove(&tx);
        self.finalised_txs.insert(tx);
        let account = self.accounts.entry(client).or_default();
        account.held -= amount;
        account.available += amount;
    }

    fn chargeback(&mut self, client: u16, tx: u32) {
        let Some(stored) = self.transactions.get_mut(&tx) else {
            return;
        };
        if stored.client != client || !stored.disputed {
            return;
        }
        let amount = stored.amount;
        // same finalisation pattern as resolve: retire the tx ID to block reuse
        self.transactions.remove(&tx);
        self.finalised_txs.insert(tx);
        let account = self.accounts.entry(client).or_default();
        account.held -= amount;
        account.locked = true;
    }
}

#[cfg(test)]
impl PaymentsEngine {
    fn assert_invariants(&self) {
        for (&client, acc) in &self.accounts {
            // total() is defined as available + held, so this catches any future
            // cached-total field that could drift out of sync
            assert_eq!(
                acc.total(),
                acc.available + acc.held,
                "client {client}: total() != available + held"
            );
            // held is only ever increased by dispute and decreased by resolve/chargeback;
            // each path is guarded so held must never go negative
            assert!(
                acc.held >= Decimal::ZERO,
                "client {client}: held is negative ({})",
                acc.held
            );
        }
        // a tx is moved from transactions → finalised_txs on resolve/chargeback;
        // it must never exist in both simultaneously
        for id in &self.finalised_txs {
            assert!(
                !self.transactions.contains_key(id),
                "tx {id} exists in both transactions and finalised_txs"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn engine_from_csv(input: &str) -> PaymentsEngine {
        let mut engine = PaymentsEngine::new();
        let mut rdr = csv::ReaderBuilder::new()
            .trim(csv::Trim::All)
            .flexible(true)
            .from_reader(input.as_bytes());
        for result in rdr.deserialize::<crate::model::TransactionRecord>() {
            if let Ok(record) = result {
                engine.apply(record);
            }
        }
        engine.assert_invariants();
        engine
    }

    fn acc(engine: &PaymentsEngine, client: u16) -> &Account {
        engine.accounts.get(&client).expect("account not found")
    }

    #[test]
    fn deposit_increases_available_and_total() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0");
        let a = acc(&e, 1);
        assert_eq!(a.available, d("100.0"));
        assert_eq!(a.held, Decimal::ZERO);
        assert_eq!(a.total(), d("100.0"));
        assert!(!a.locked);
    }

    #[test]
    fn withdrawal_decreases_available() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\nwithdrawal,1,2,30.0");
        assert_eq!(acc(&e, 1).available, d("70.0"));
    }

    #[test]
    fn withdrawal_fails_if_insufficient_funds() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,50.0\nwithdrawal,1,2,100.0");
        assert_eq!(acc(&e, 1).available, d("50.0"));
    }

    #[test]
    fn withdrawal_fails_on_locked_account() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nchargeback,1,1,\nwithdrawal,1,2,10.0",
        );
        let a = acc(&e, 1);
        assert!(a.locked);
        assert_eq!(a.available, Decimal::ZERO);
    }

    #[test]
    fn dispute_moves_funds_to_held() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,");
        let a = acc(&e, 1);
        assert_eq!(a.available, Decimal::ZERO);
        assert_eq!(a.held, d("100.0"));
        assert_eq!(a.total(), d("100.0"));
    }

    #[test]
    fn resolve_releases_held_funds() {
        let e =
            engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nresolve,1,1,");
        let a = acc(&e, 1);
        assert_eq!(a.available, d("100.0"));
        assert_eq!(a.held, Decimal::ZERO);
        assert!(!a.locked);
    }

    #[test]
    fn chargeback_removes_held_and_locks_account() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nchargeback,1,1,",
        );
        let a = acc(&e, 1);
        assert_eq!(a.available, Decimal::ZERO);
        assert_eq!(a.held, Decimal::ZERO);
        assert_eq!(a.total(), Decimal::ZERO);
        assert!(a.locked);
    }

    #[test]
    fn duplicate_tx_id_is_rejected() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\ndeposit,1,1,50.0");
        assert_eq!(acc(&e, 1).available, d("100.0"));
    }

    #[test]
    fn dispute_on_unknown_tx_is_ignored() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,999,");
        let a = acc(&e, 1);
        assert_eq!(a.available, d("100.0"));
        assert_eq!(a.held, Decimal::ZERO);
    }

    #[test]
    fn chargeback_without_prior_dispute_is_ignored() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\nchargeback,1,1,");
        let a = acc(&e, 1);
        assert_eq!(a.available, d("100.0"));
        assert!(!a.locked);
    }

    #[test]
    fn tx_id_reuse_after_resolve_is_rejected() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nresolve,1,1,\ndeposit,1,1,50.0",
        );
        assert_eq!(acc(&e, 1).available, d("100.0"));
    }

    #[test]
    fn tx_id_reuse_after_chargeback_is_rejected() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nchargeback,1,1,\ndeposit,2,1,50.0",
        );
        assert!(!e.accounts.contains_key(&2));
    }

    #[test]
    fn cross_client_dispute_is_ignored() {
        let e = engine_from_csv("type,client,tx,amount\ndeposit,1,1,100.0\ndispute,2,1,");
        let a = acc(&e, 1);
        assert_eq!(a.available, d("100.0"));
        assert_eq!(a.held, Decimal::ZERO);
    }

    // --- property tests ---

    mod property {
        use super::*;
        use proptest::prelude::*;

        fn arb_amount() -> impl Strategy<Value = Decimal> {
            // 0.01 to 1000.00 — stays well within Decimal precision and avoids overflow
            // when summing up to ~20 deposits in the sequence test
            (1u64..=100_000u64).prop_map(|n| Decimal::new(n as i64, 2))
        }

        #[derive(Debug, Clone)]
        enum Op {
            Deposit,
            Withdrawal,
            Dispute,
            Resolve,
            Chargeback,
        }

        fn arb_op() -> impl Strategy<Value = Op> {
            // weight deposits higher so generated sequences contain more valid state
            prop_oneof![
                3 => Just(Op::Deposit),
                2 => Just(Op::Withdrawal),
                2 => Just(Op::Dispute),
                1 => Just(Op::Resolve),
                1 => Just(Op::Chargeback),
            ]
        }

        proptest! {
            #[test]
            fn deposit_increases_available_by_exact_amount(amount in arb_amount()) {
                let csv = format!("type,client,tx,amount\ndeposit,1,1,{amount}");
                let e = engine_from_csv(&csv);
                prop_assert_eq!(e.accounts[&1u16].available, amount);
                prop_assert_eq!(e.accounts[&1u16].held, Decimal::ZERO);
            }

            #[test]
            fn deposit_then_full_withdrawal_zeroes_balance(amount in arb_amount()) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{amount}\nwithdrawal,1,2,{amount}"
                );
                let e = engine_from_csv(&csv);
                let a = &e.accounts[&1u16];
                prop_assert_eq!(a.available, Decimal::ZERO);
                prop_assert_eq!(a.total(), Decimal::ZERO);
            }

            #[test]
            fn withdrawal_never_overdrafts_available(
                deposit in arb_amount(),
                withdrawal in arb_amount()
            ) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{deposit}\nwithdrawal,1,2,{withdrawal}"
                );
                let e = engine_from_csv(&csv);
                prop_assert!(e.accounts[&1u16].available >= Decimal::ZERO);
            }

            #[test]
            fn dispute_preserves_total(amount in arb_amount()) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{amount}\ndispute,1,1,"
                );
                let e = engine_from_csv(&csv);
                let a = &e.accounts[&1u16];
                prop_assert_eq!(a.total(), amount);
                prop_assert_eq!(a.held, amount);
                prop_assert_eq!(a.available, Decimal::ZERO);
            }

            #[test]
            fn dispute_then_resolve_is_a_no_op(amount in arb_amount()) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{amount}\ndispute,1,1,\nresolve,1,1,"
                );
                let e = engine_from_csv(&csv);
                let a = &e.accounts[&1u16];
                prop_assert_eq!(a.available, amount);
                prop_assert_eq!(a.held, Decimal::ZERO);
                prop_assert!(!a.locked);
            }

            #[test]
            fn chargeback_always_locks_account(amount in arb_amount()) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{amount}\ndispute,1,1,\nchargeback,1,1,"
                );
                let e = engine_from_csv(&csv);
                prop_assert!(e.accounts[&1u16].locked);
                prop_assert_eq!(e.accounts[&1u16].held, Decimal::ZERO);
            }

            #[test]
            fn multiple_deposits_accumulate_correctly(
                a1 in arb_amount(),
                a2 in arb_amount(),
                a3 in arb_amount()
            ) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{a1}\ndeposit,1,2,{a2}\ndeposit,1,3,{a3}"
                );
                let e = engine_from_csv(&csv);
                prop_assert_eq!(e.accounts[&1u16].available, a1 + a2 + a3);
            }

            #[test]
            fn duplicate_tx_id_counts_only_first_deposit(
                a1 in arb_amount(),
                a2 in arb_amount()
            ) {
                let csv = format!(
                    "type,client,tx,amount\ndeposit,1,1,{a1}\ndeposit,1,1,{a2}"
                );
                let e = engine_from_csv(&csv);
                prop_assert_eq!(e.accounts[&1u16].available, a1);
            }

            // most powerful test: arbitrary sequences of all op types across multiple
            // clients with overlapping tx IDs — engine must never violate invariants
            #[test]
            fn invariants_hold_after_arbitrary_op_sequence(
                ops in proptest::collection::vec(
                    (arb_op(), 1u16..=3u16, 1u32..=10u32, arb_amount()),
                    1..20
                )
            ) {
                let header = "type,client,tx,amount";
                let rows: Vec<String> = ops.iter().map(|(op, client, tx, amount)| {
                    match op {
                        Op::Deposit    => format!("deposit,{client},{tx},{amount}"),
                        Op::Withdrawal => format!("withdrawal,{client},{tx},{amount}"),
                        Op::Dispute    => format!("dispute,{client},{tx},"),
                        Op::Resolve    => format!("resolve,{client},{tx},"),
                        Op::Chargeback => format!("chargeback,{client},{tx},"),
                    }
                }).collect();
                let csv = format!("{header}\n{}", rows.join("\n"));
                // engine_from_csv calls assert_invariants() on the result
                engine_from_csv(&csv);
            }
        }
    }

    // --- invariant tests ---

    #[test]
    fn invariant_held_never_negative_after_chargeback() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nchargeback,1,1,",
        );
        // assert_invariants is called inside engine_from_csv; this asserts the
        // final visible state too
        assert_eq!(acc(&e, 1).held, Decimal::ZERO);
    }

    #[test]
    fn invariant_held_never_negative_after_resolve() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nresolve,1,1,",
        );
        assert_eq!(acc(&e, 1).held, Decimal::ZERO);
    }

    #[test]
    fn invariant_tx_not_in_both_maps_after_resolve() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nresolve,1,1,",
        );
        // tx 1 must be in finalised_txs and absent from transactions
        assert!(e.finalised_txs.contains(&1));
        assert!(!e.transactions.contains_key(&1));
    }

    #[test]
    fn invariant_tx_not_in_both_maps_after_chargeback() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nchargeback,1,1,",
        );
        assert!(e.finalised_txs.contains(&1));
        assert!(!e.transactions.contains_key(&1));
    }

    #[test]
    fn invariant_held_correct_across_multiple_disputes() {
        // two separate deposits, both disputed; held must equal sum of both amounts
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,40.0\ndeposit,1,2,60.0\ndispute,1,1,\ndispute,1,2,",
        );
        let a = acc(&e, 1);
        assert_eq!(a.held, d("100.0"));
        assert!(a.held >= Decimal::ZERO);
    }

    #[test]
    fn invariant_held_correct_after_partial_resolve() {
        // two disputes, one resolved — held must only contain the remaining disputed amount
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,40.0\ndeposit,1,2,60.0\ndispute,1,1,\ndispute,1,2,\nresolve,1,1,",
        );
        let a = acc(&e, 1);
        assert_eq!(a.held, d("60.0"));
        assert!(a.held >= Decimal::ZERO);
    }

    #[test]
    fn invariant_multi_client_independent() {
        // invariants must hold for every client independently
        let e = engine_from_csv(
            "type,client,tx,amount\n\
             deposit,1,1,100.0\ndeposit,2,2,200.0\n\
             dispute,1,1,\ndispute,2,2,\n\
             chargeback,1,1,\nresolve,2,2,",
        );
        e.assert_invariants();
        assert!(acc(&e, 1).locked);
        assert!(!acc(&e, 2).locked);
        assert_eq!(acc(&e, 1).held, Decimal::ZERO);
        assert_eq!(acc(&e, 2).held, Decimal::ZERO);
    }
}
