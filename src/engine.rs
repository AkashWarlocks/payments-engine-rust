use crate::error::Result;
use crate::model::{Account, AccountRecord, StoredTx, TransactionRecord, TxType};
use csv::ReaderBuilder;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::io::Write;

pub struct PaymentsEngine {
    accounts: HashMap<u16, Account>,
    transactions: HashMap<u32, StoredTx>,
}

impl PaymentsEngine {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            transactions: HashMap::new(),
        }
    }

    pub fn process_file(&mut self, path: &str) -> Result<()> {
        let file = std::fs::File::open(path)?;
        let mut rdr = ReaderBuilder::new()
            .trim(csv::Trim::All)
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
                    total: format!("{:.4}", (available + held).round_dp(4)),
                    locked: acc.locked,
                }
            })
            .collect();
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
        if amount <= Decimal::ZERO { return; }
        if self.transactions.contains_key(&tx) { return; }
        let account = self.accounts.entry(client).or_default();
        if account.locked { return; }
        account.available += amount;
        self.transactions.insert(tx, StoredTx { client, amount, disputed: false });
    }

    fn withdrawal(&mut self, client: u16, tx: u32, amount: Option<Decimal>) {
        let Some(amount) = amount else { return };
        if amount <= Decimal::ZERO { return; }
        if self.transactions.contains_key(&tx) { return; }
        let account = self.accounts.entry(client).or_default();
        if account.locked { return; }
        if account.available < amount { return; }
        account.available -= amount;
        self.transactions.insert(tx, StoredTx { client, amount, disputed: false });
    }

    fn dispute(&mut self, client: u16, tx: u32) {
        let Some(stored) = self.transactions.get_mut(&tx) else { return };
        if stored.client != client || stored.disputed { return; }
        stored.disputed = true;
        let amount = stored.amount;
        let account = self.accounts.entry(client).or_default();
        account.available -= amount;
        account.held += amount;
    }

    fn resolve(&mut self, client: u16, tx: u32) {
        let Some(stored) = self.transactions.get_mut(&tx) else { return };
        if stored.client != client || !stored.disputed { return; }
        let amount = stored.amount;
        self.transactions.remove(&tx);
        let account = self.accounts.entry(client).or_default();
        account.held -= amount;
        account.available += amount;
    }

    fn chargeback(&mut self, client: u16, tx: u32) {
        let Some(stored) = self.transactions.get_mut(&tx) else { return };
        if stored.client != client || !stored.disputed { return; }
        let amount = stored.amount;
        self.transactions.remove(&tx);
        let account = self.accounts.entry(client).or_default();
        account.held -= amount;
        account.locked = true;
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
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\nwithdrawal,1,2,30.0",
        );
        assert_eq!(acc(&e, 1).available, d("70.0"));
    }

    #[test]
    fn withdrawal_fails_if_insufficient_funds() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,50.0\nwithdrawal,1,2,100.0",
        );
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
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,1,1,\nresolve,1,1,",
        );
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
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndeposit,1,1,50.0",
        );
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
    fn cross_client_dispute_is_ignored() {
        let e = engine_from_csv(
            "type,client,tx,amount\ndeposit,1,1,100.0\ndispute,2,1,",
        );
        let a = acc(&e, 1);
        assert_eq!(a.available, d("100.0"));
        assert_eq!(a.held, Decimal::ZERO);
    }
}
