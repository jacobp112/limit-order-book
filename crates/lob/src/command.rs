//! Engine input ([`Command`]) and output ([`Event`]).
//!
//! A command journal plus the engine's initial state fully determines the
//! event stream, so events carry everything an observer needs to rebuild
//! per-order accounting without inspecting the book.

use crate::types::{OrderId, OrderType, Price, Qty, Seq, Side};

/// An instruction to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// Enter a new order.
    Submit {
        /// Buy or sell.
        side: Side,
        /// Limit, IOC or market.
        kind: OrderType,
        /// Quantity in lots.
        qty: Qty,
    },
    /// Remove a resting order.
    Cancel {
        /// Order to cancel.
        id: OrderId,
    },
    /// Change a resting order's open quantity and optionally its price.
    Amend {
        /// Order to amend.
        id: OrderId,
        /// New price; `None` keeps the current price.
        price: Option<Price>,
        /// New open quantity.
        qty: Qty,
    },
}

impl Command {
    /// Checks that depend only on the command itself, not on book state.
    ///
    /// # Errors
    ///
    /// Returns the first reason the command's fields are unacceptable.
    pub fn check_fields(&self) -> Result<(), RejectReason> {
        let (qty, price) = match *self {
            Self::Submit { kind, qty, .. } => (Some(qty), kind.limit()),
            Self::Cancel { .. } => (None, None),
            Self::Amend { price, qty, .. } => (Some(qty), price),
        };
        if let Some(qty) = qty {
            if qty.is_zero() {
                return Err(RejectReason::ZeroQuantity);
            }
            if !qty.is_valid() {
                return Err(RejectReason::QuantityTooLarge);
            }
        }
        if price.is_some_and(|p| !p.is_valid()) {
            return Err(RejectReason::PriceOutOfRange);
        }
        Ok(())
    }
}

/// Why a command was rejected. A rejected command has no other effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// Quantity was zero.
    ZeroQuantity,
    /// Quantity exceeded [`crate::types::MAX_QTY`].
    QuantityTooLarge,
    /// Price was outside `1..=MAX_PRICE`.
    PriceOutOfRange,
    /// The id was never issued or the order is no longer resting.
    UnknownOrder,
    /// An amend that would change neither price nor quantity.
    AmendNoChange,
}

/// Why open quantity left the book without trading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CancelReason {
    /// Explicit cancel command.
    User,
    /// Unfilled remainder of an IOC order.
    IocRemainder,
    /// Unfilled remainder of a market order.
    NoLiquidity,
}

/// Whether an amend kept the order's queue position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Same position in the queue.
    Kept,
    /// Moved to the back of the queue with a new sequence number.
    Lost,
}

/// Something the engine did in response to a command.
///
/// Per command the order is: `Rejected` alone; or `Accepted`/`Amended`,
/// then any `Trade`s (each maker that completes is followed by its
/// `Filled`), then exactly one of `Filled` (taker), `Rested` or `Cancelled`
/// for the incoming order's remainder. A same-price quantity decrease is
/// just `Amended`. A cancel is just `Cancelled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// A submitted order passed validation and was assigned an id.
    Accepted {
        /// New order id.
        id: OrderId,
        /// Order side.
        side: Side,
        /// Order type.
        kind: OrderType,
        /// Submitted quantity.
        qty: Qty,
        /// Priority sequence number.
        seq: Seq,
    },
    /// The command was rejected and had no effect.
    Rejected {
        /// Target order, for cancel and amend.
        id: Option<OrderId>,
        /// Reason.
        reason: RejectReason,
    },
    /// A resting order was amended.
    Amended {
        /// Order id.
        id: OrderId,
        /// Price after the amend.
        price: Price,
        /// Open quantity before the amend.
        old_qty: Qty,
        /// Open quantity after the amend, before any resulting trades.
        qty: Qty,
        /// Sequence number after the amend.
        seq: Seq,
        /// Whether the queue position was kept.
        priority: Priority,
    },
    /// Two orders traded.
    Trade {
        /// Resting order.
        maker: OrderId,
        /// Incoming order.
        taker: OrderId,
        /// Side of the incoming order.
        taker_side: Side,
        /// Execution price (the maker's price).
        price: Price,
        /// Traded quantity.
        qty: Qty,
    },
    /// An order's open quantity reached zero by trading.
    Filled {
        /// Order id.
        id: OrderId,
    },
    /// An order was placed at the back of a price level.
    Rested {
        /// Order id.
        id: OrderId,
        /// Side.
        side: Side,
        /// Level price.
        price: Price,
        /// Open quantity placed.
        qty: Qty,
        /// Priority sequence number.
        seq: Seq,
    },
    /// Open quantity was removed without trading.
    Cancelled {
        /// Order id.
        id: OrderId,
        /// Quantity removed.
        qty: Qty,
        /// Reason.
        reason: CancelReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MAX_PRICE, MAX_QTY};

    fn submit(kind: OrderType, qty: u64) -> Command {
        Command::Submit {
            side: Side::Buy,
            kind,
            qty: Qty(qty),
        }
    }

    #[test]
    fn valid_commands_pass_field_checks() {
        assert_eq!(
            submit(OrderType::Limit { price: Price(10) }, 5).check_fields(),
            Ok(())
        );
        assert_eq!(submit(OrderType::Market, MAX_QTY).check_fields(), Ok(()));
        assert_eq!(Command::Cancel { id: OrderId(0) }.check_fields(), Ok(()));
        let amend = Command::Amend {
            id: OrderId(1),
            price: None,
            qty: Qty(1),
        };
        assert_eq!(amend.check_fields(), Ok(()));
    }

    #[test]
    fn quantity_is_checked_before_price() {
        let cmd = submit(OrderType::Limit { price: Price(0) }, 0);
        assert_eq!(cmd.check_fields(), Err(RejectReason::ZeroQuantity));
    }

    #[test]
    fn out_of_range_fields_are_rejected() {
        let limit = |p| OrderType::Limit { price: Price(p) };
        let ioc = |p| OrderType::Ioc { price: Price(p) };
        assert_eq!(
            submit(OrderType::Market, MAX_QTY + 1).check_fields(),
            Err(RejectReason::QuantityTooLarge)
        );
        assert_eq!(
            submit(limit(0), 1).check_fields(),
            Err(RejectReason::PriceOutOfRange)
        );
        assert_eq!(
            submit(ioc(-5), 1).check_fields(),
            Err(RejectReason::PriceOutOfRange)
        );
        assert_eq!(
            submit(limit(MAX_PRICE + 1), 1).check_fields(),
            Err(RejectReason::PriceOutOfRange)
        );
    }

    #[test]
    fn amend_fields_are_checked() {
        let amend = |price, qty| Command::Amend {
            id: OrderId(1),
            price,
            qty: Qty(qty),
        };
        assert_eq!(
            amend(None, 0).check_fields(),
            Err(RejectReason::ZeroQuantity)
        );
        assert_eq!(
            amend(Some(Price(0)), 3).check_fields(),
            Err(RejectReason::PriceOutOfRange)
        );
        assert_eq!(
            amend(None, u64::MAX).check_fields(),
            Err(RejectReason::QuantityTooLarge)
        );
    }
}
