//! Integration tests for all 28 WalletInterface HTTP endpoints.
//!
//! These tests spin up a real HTTP server with a fresh wallet backed by
//! a temporary SQLite database. They exercise every endpoint and verify
//! the response shape.
//!
//! One module per group of endpoints; `support` builds the worlds.

mod auth;
mod certificates;
mod crypto;
mod discovery;
mod e2e;
mod e2e_broadcast;
mod key_linkage;
mod status;
mod support;
mod transactions;
