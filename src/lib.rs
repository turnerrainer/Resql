//! Resql-on-Rust: SQL-files-as-REST-endpoints microservice.
//!
//! Public API is intentionally narrow; the binary in `src/main.rs` is the
//! only supported consumer. Integration tests reach in via re-exports below.

pub mod config;
pub mod db;
pub mod error;
pub mod health;
pub mod loader;
pub mod query;
pub mod server;
