//! Primitive domain types.
//!
//! Prices and quantities are integers (ticks and lots). The newtypes can hold
//! out-of-range values so that bad input can be represented and rejected by
//! the engine; code past validation may assume [`Price::is_valid`] and
//! [`Qty::is_valid`] hold.

use std::fmt;

/// Largest accepted price, in ticks.
pub const MAX_PRICE: i64 = 1_000_000_000;

/// Largest accepted order quantity, in lots.
///
/// With at most `u32::MAX` resting orders, a level total is bounded by
/// `MAX_QTY * u32::MAX < u64::MAX`, so level accounting cannot overflow.
pub const MAX_QTY: u64 = 1_000_000_000;

/// A price in ticks. Valid prices are `1..=MAX_PRICE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub i64);

impl Price {
    /// Whether the price is inside the accepted range.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.0 >= 1 && self.0 <= MAX_PRICE
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A quantity in lots. Valid order quantities are `1..=MAX_QTY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Qty(pub u64);

impl Qty {
    /// The zero quantity.
    pub const ZERO: Self = Self(0);

    /// Whether the quantity is acceptable for an order.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.0 >= 1 && self.0 <= MAX_QTY
    }

    /// Whether the quantity is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Qty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Engine-assigned order identifier. The first order is id 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderId(pub u64);

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Logical time. A resting order's sequence number is its queue position:
/// within a price level, lower sequence numbers trade first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub u64);

/// Order side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// Bid.
    Buy,
    /// Ask.
    Sell,
}

impl Side {
    /// The side this one trades against.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }

    /// Whether an order on this side with limit `limit` can trade against a
    /// resting order at `resting`.
    #[must_use]
    pub fn crosses(self, limit: Price, resting: Price) -> bool {
        match self {
            Self::Buy => limit >= resting,
            Self::Sell => limit <= resting,
        }
    }
}

/// How an incoming order treats its unfilled remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderType {
    /// Trade while crossing, then rest the remainder at `price`.
    Limit {
        /// Limit price.
        price: Price,
    },
    /// Trade while crossing, then cancel the remainder.
    Ioc {
        /// Limit price.
        price: Price,
    },
    /// Trade at any price, then cancel the remainder.
    Market,
}

impl OrderType {
    /// The limit price, if the type has one.
    #[must_use]
    pub const fn limit(self) -> Option<Price> {
        match self {
            Self::Limit { price } | Self::Ioc { price } => Some(price),
            Self::Market => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_bounds() {
        assert!(!Price(0).is_valid());
        assert!(!Price(-1).is_valid());
        assert!(Price(1).is_valid());
        assert!(Price(MAX_PRICE).is_valid());
        assert!(!Price(MAX_PRICE + 1).is_valid());
        assert!(!Price(i64::MIN).is_valid());
    }

    #[test]
    fn qty_bounds() {
        assert!(!Qty(0).is_valid());
        assert!(Qty(1).is_valid());
        assert!(Qty(MAX_QTY).is_valid());
        assert!(!Qty(MAX_QTY + 1).is_valid());
        assert!(!Qty(u64::MAX).is_valid());
    }

    #[test]
    fn level_total_cannot_overflow() {
        assert!(u128::from(MAX_QTY) * u128::from(u32::MAX) < u128::from(u64::MAX));
    }

    #[test]
    fn crossing_is_inclusive_and_side_dependent() {
        let p = Price(100);
        assert!(Side::Buy.crosses(Price(100), p));
        assert!(Side::Buy.crosses(Price(101), p));
        assert!(!Side::Buy.crosses(Price(99), p));
        assert!(Side::Sell.crosses(Price(100), p));
        assert!(Side::Sell.crosses(Price(99), p));
        assert!(!Side::Sell.crosses(Price(101), p));
    }

    #[test]
    fn opposite_is_an_involution() {
        for side in [Side::Buy, Side::Sell] {
            assert_ne!(side.opposite(), side);
            assert_eq!(side.opposite().opposite(), side);
        }
    }

    #[test]
    fn market_has_no_limit() {
        assert_eq!(OrderType::Market.limit(), None);
        assert_eq!(OrderType::Ioc { price: Price(5) }.limit(), Some(Price(5)));
        assert_eq!(OrderType::Limit { price: Price(7) }.limit(), Some(Price(7)));
    }
}
