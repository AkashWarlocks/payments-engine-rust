//! CLI entry point — reads a transaction CSV and writes final account states to stdout.

mod engine;
mod error;
mod model;

use engine::PaymentsEngine;
use std::{env, io, process};

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
