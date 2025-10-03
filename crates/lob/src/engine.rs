//! Validation, id/sequence assignment and event emission around the book.

use crate::book::OrderBook;
use crate::command::{CancelReason, Command, Event, RejectReason};
use crate::types::{OrderId, OrderType, Qty, Seq, Side};

/// A single-instrument matching engine.
///
/// All state that affects output lives here: the book and the id and
/// sequence counters. There is no clock or randomness.
#[derive(Debug)]
pub struct MatchingEngine {
    book: OrderBook,
    next_id: u64,
    next_seq: u64,
}

impl Default for MatchingEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchingEngine {
    /// An engine with an empty book. The first order gets id 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            book: OrderBook::new(),
            next_id: 1,
            next_seq: 1,
        }
    }

    /// Read-only view of the book.
    #[must_use]
    pub fn book(&self) -> &OrderBook {
        &self.book
    }

    /// Submits a new order, appending the resulting events to `out`.
    pub fn submit(&mut self, side: Side, kind: OrderType, qty: Qty, out: &mut Vec<Event>) {
        if let Err(reason) = (Command::Submit { side, kind, qty }).check_fields() {
            out.push(Event::Rejected { id: None, reason });
            return;
        }
        let id = self.take_id();
        let seq = self.take_seq();
        out.push(Event::Accepted {
            id,
            side,
            kind,
            qty,
            seq,
        });

        let remaining = self.book.match_incoming(id, side, kind.limit(), qty, out);
        if remaining.is_zero() {
            out.push(Event::Filled { id });
            return;
        }
        match kind {
            OrderType::Limit { price } => {
                self.book.insert(id, side, price, remaining, seq);
                out.push(Event::Rested {
                    id,
                    side,
                    price,
                    qty: remaining,
                    seq,
                });
            }
            OrderType::Ioc { .. } => out.push(Event::Cancelled {
                id,
                qty: remaining,
                reason: CancelReason::IocRemainder,
            }),
            OrderType::Market => out.push(Event::Cancelled {
                id,
                qty: remaining,
                reason: CancelReason::NoLiquidity,
            }),
        }
    }

    /// Cancels a resting order, appending the resulting event to `out`.
    pub fn cancel(&mut self, id: OrderId, out: &mut Vec<Event>) {
        match self.book.remove(id) {
            Some(order) => out.push(Event::Cancelled {
                id,
                qty: order.qty,
                reason: CancelReason::User,
            }),
            None => out.push(Event::Rejected {
                id: Some(id),
                reason: RejectReason::UnknownOrder,
            }),
        }
    }

    fn take_id(&mut self) -> OrderId {
        let id = OrderId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("order id space exhausted");
        id
    }

    fn take_seq(&mut self) -> Seq {
        let seq = Seq(self.next_seq);
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .expect("sequence space exhausted");
        seq
    }
}
