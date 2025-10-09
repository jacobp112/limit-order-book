//! A deterministic in-memory limit order book with price-time priority matching.
//!
//! See `docs/requirements.md` for scope and invariants and `docs/design.md`
//! for the data structures and their complexity.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod book;
pub mod command;
pub mod engine;
pub mod invariants;
pub mod journal;
mod level;
pub mod snapshot;
pub mod types;

pub use book::{LevelSummary, OrderBook, RestingOrder};
pub use command::{CancelReason, Command, Event, Priority, RejectReason};
pub use engine::MatchingEngine;
pub use snapshot::{Snapshot, SnapshotError};
pub use types::{MAX_PRICE, MAX_QTY, OrderId, OrderType, Price, Qty, Seq, Side};
