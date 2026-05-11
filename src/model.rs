use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Transaction types as they appear in the input CSV.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum TxType {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// A single row deserialised from the input CSV.
///
/// `amount` is `Option` because dispute, resolve, and chargeback rows
/// legally omit the amount column — the engine uses the stored deposit amount instead.
#[derive(Debug, Deserialize)]
pub struct TransactionRecord {
    #[serde(rename = "type")]
    pub tx_type: TxType,
    pub client: u16,
    pub tx: u32,
    pub amount: Option<Decimal>,
}

/// Minimal record kept in memory for a transaction that is eligible for dispute.
///
/// Only deposits are stored (withdrawals are stored too but cannot be disputed).
/// Dispute/resolve/chargeback are never stored — they reference existing entries.
pub struct StoredTx {
    pub client: u16,
    pub amount: Decimal,
    pub disputed: bool,
    pub tx_type: TxType,
}

/// Live balance state for one client.
///
/// `available` is the spendable balance; `held` is funds frozen under an active dispute.
///
/// Note: `available` can go negative if a deposit is disputed after a partial withdrawal
/// has already reduced the balance below the deposit amount. This is a known limitation —
/// see README for details.
#[derive(Default)]
pub struct Account {
    pub available: Decimal,
    pub held: Decimal,
    pub locked: bool,
}

impl Account {
    /// Sum of spendable and held funds.
    pub fn total(&self) -> Decimal {
        self.available + self.held
    }
}

/// CSV output record — one row per client in the final report.
///
/// Amounts are pre-formatted as strings to 4 decimal places.
#[derive(Serialize)]
pub struct AccountRecord {
    pub client: u16,
    pub available: String,
    pub held: String,
    pub total: String,
    pub locked: bool,
}
