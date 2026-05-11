#![no_main]

use libfuzzer_sys::fuzz_target;
use payments_engine::model::TransactionRecord;

// Fuzz the CSV deserialiser for a single row independently of the engine.
// Catches panics in serde/csv parsing before they reach the transaction logic.
fuzz_target!(|data: &[u8]| {
    let mut rdr = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(data);
    for result in rdr.deserialize::<TransactionRecord>() {
        // Ok or Err are fine — a panic is a bug
        let _ = result;
    }
});
