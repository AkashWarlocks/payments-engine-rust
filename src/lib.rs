//! Core library for the payments engine.
//!
//! Exposes [`PaymentsEngine`] for processing CSV transaction streams
//! and writing final account states.

pub mod engine;
pub mod error;
pub mod model;

pub use engine::PaymentsEngine;
