use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum TxType {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

#[derive(Debug, Deserialize)]
pub struct TransactionRecord {
    #[serde(rename = "type")]
    pub tx_type: TxType,
    pub client: u16,
    pub tx: u32,
    pub amount: Option<Decimal>,
}

pub struct StoredTx {
    pub client: u16,
    pub amount: Decimal,
    pub disputed: bool,
}

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

#[derive(Serialize)]
pub struct AccountRecord {
    pub client: u16,
    pub available: String,
    pub held: String,
    pub total: String,
    pub locked: bool,
}
