#![no_main]

use libfuzzer_sys::fuzz_target;
use payments_engine::PaymentsEngine;

// Feed arbitrary bytes as a CSV stream to the full engine pipeline.
// The engine must never panic — Ok or Err are both acceptable outcomes.
fuzz_target!(|data: &[u8]| {
    let mut engine = PaymentsEngine::new();
    let _ = engine.process_reader(data);
});
