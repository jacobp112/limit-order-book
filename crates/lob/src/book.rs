//! Both sides of the book, the order arena and the id index.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::BuildHasherDefault;

use crate::command::Event;
use crate::invariants::Violation;
use crate::level::{Arena, Handle, Level, Node};
use crate::types::{OrderId, Price, Qty, Seq, Side};

/// `DefaultHasher::default()` uses fixed keys, so the index behaves the same
/// on every run. The index is only ever used for lookup, never iterated.
type IdIndex = HashMap<OrderId, Handle, BuildHasherDefault<DefaultHasher>>;

/// A resting order as seen from outside the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestingOrder {
    /// Order id.
    pub id: OrderId,
    /// Side.
    pub side: Side,
    /// Price level the order rests at.
    pub price: Price,
    /// Open quantity.
    pub qty: Qty,
    /// Priority sequence number.
    pub seq: Seq,
}

impl From<&Node> for RestingOrder {
    fn from(n: &Node) -> Self {
        Self {
            id: n.id,
            side: n.side,
            price: n.price,
            qty: n.open,
            seq: n.seq,
        }
    }
}

/// Aggregate view of one price level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelSummary {
    /// Level price.
    pub price: Price,
    /// Total open quantity at the level.
    pub qty: Qty,
    /// Number of orders at the level.
    pub orders: u32,
}

/// The resting orders of a single instrument.
#[derive(Debug, Default)]
pub struct OrderBook {
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,
    arena: Arena,
    index: IdIndex,
}

impl OrderBook {
    /// An empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Highest bid price.
    #[must_use]
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.last_key_value().map(|(p, _)| *p)
    }

    /// Lowest ask price.
    #[must_use]
    pub fn best_ask(&self) -> Option<Price> {
        self.asks.first_key_value().map(|(p, _)| *p)
    }

    /// Best price on `side`.
    #[must_use]
    pub fn best(&self, side: Side) -> Option<Price> {
        match side {
            Side::Buy => self.best_bid(),
            Side::Sell => self.best_ask(),
        }
    }

    /// Number of resting orders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether no orders are resting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// A resting order by id.
    #[must_use]
    pub fn order(&self, id: OrderId) -> Option<RestingOrder> {
        self.index.get(&id).map(|&h| self.arena.get(h).into())
    }

    /// Up to `max_levels` levels of `side`, best price first.
    #[must_use]
    pub fn depth(&self, side: Side, max_levels: usize) -> Vec<LevelSummary> {
        let summary = |(p, l): (&Price, &Level)| LevelSummary {
            price: *p,
            qty: l.total(),
            orders: l.count(),
        };
        match side {
            Side::Buy => self
                .bids
                .iter()
                .rev()
                .take(max_levels)
                .map(summary)
                .collect(),
            Side::Sell => self.asks.iter().take(max_levels).map(summary).collect(),
        }
    }

    /// The orders resting at `price` on `side`, in time priority.
    #[must_use]
    pub fn level_orders(&self, side: Side, price: Price) -> Vec<RestingOrder> {
        self.levels(side)
            .get(&price)
            .map_or_else(Vec::new, |level| {
                level
                    .iter(&self.arena)
                    .map(|h| self.arena.get(h).into())
                    .collect()
            })
    }

    /// Every resting order on `side`, best price first and in time priority
    /// within each price.
    #[must_use]
    pub fn resting(&self, side: Side) -> Vec<RestingOrder> {
        let level_orders = |level: &Level| {
            level
                .iter(&self.arena)
                .map(|h| RestingOrder::from(self.arena.get(h)))
                .collect::<Vec<_>>()
        };
        match side {
            Side::Buy => self.bids.values().rev().flat_map(level_orders).collect(),
            Side::Sell => self.asks.values().flat_map(level_orders).collect(),
        }
    }

    /// Places a new order at the back of its price level.
    ///
    /// The caller guarantees `id` is not already resting and that price and
    /// quantity are valid.
    pub(crate) fn insert(&mut self, id: OrderId, side: Side, price: Price, qty: Qty, seq: Seq) {
        debug_assert!(price.is_valid() && qty.is_valid());
        let h = self.arena.alloc(id, side, price, qty, seq);
        let previous = self.index.insert(id, h);
        assert!(previous.is_none(), "order {id} inserted twice");
        let levels = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        levels
            .entry(price)
            .or_default()
            .push_back(&mut self.arena, h);
    }

    /// Removes a resting order, returning its final state.
    pub(crate) fn remove(&mut self, id: OrderId) -> Option<RestingOrder> {
        let h = self.index.remove(&id)?;
        let (side, price) = {
            let n = self.arena.get(h);
            (n.side, n.price)
        };
        let levels = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = levels.get_mut(&price).expect("indexed order has a level");
        level.unlink(&mut self.arena, h);
        if level.is_empty() {
            levels.remove(&price);
        }
        Some((&self.arena.free(h)).into())
    }

    /// Lowers a resting order's open quantity to `qty` without moving it.
    ///
    /// The caller guarantees `0 < qty < current open quantity`.
    pub(crate) fn reduce(&mut self, id: OrderId, qty: Qty) {
        let h = *self.index.get(&id).expect("reduce of resting order");
        let (side, price, open) = {
            let n = self.arena.get(h);
            (n.side, n.price, n.open)
        };
        debug_assert!(!qty.is_zero() && qty < open);
        let by = open.checked_sub(qty).expect("reduce lowers quantity");
        let levels = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        levels
            .get_mut(&price)
            .expect("indexed order has a level")
            .reduce(&mut self.arena, h, by);
    }

    /// Trades an incoming order against the opposite side in price-time
    /// priority, emitting `Trade` and maker `Filled` events. Stops when
    /// `qty` is exhausted, the opposite side is empty, or the next level no
    /// longer crosses `limit` (`None` crosses everything). Returns the
    /// unfilled remainder.
    pub(crate) fn match_incoming(
        &mut self,
        taker: OrderId,
        taker_side: Side,
        limit: Option<Price>,
        qty: Qty,
        out: &mut Vec<Event>,
    ) -> Qty {
        let mut remaining = qty;
        let levels = match taker_side {
            Side::Buy => &mut self.asks,
            Side::Sell => &mut self.bids,
        };
        while !remaining.is_zero() {
            let best = match taker_side {
                Side::Buy => levels.first_entry(),
                Side::Sell => levels.last_entry(),
            };
            let Some(mut entry) = best else { break };
            let price = *entry.key();
            if limit.is_some_and(|l| !taker_side.crosses(l, price)) {
                break;
            }
            let level = entry.get_mut();
            while let Some(h) = level.head() {
                let (maker, open) = {
                    let n = self.arena.get(h);
                    (n.id, n.open)
                };
                let fill = remaining.min(open);
                out.push(Event::Trade {
                    maker,
                    taker,
                    taker_side,
                    price,
                    qty: fill,
                });
                remaining = remaining.checked_sub(fill).expect("fill <= remaining");
                if fill == open {
                    self.index.remove(&maker);
                    level.unlink(&mut self.arena, h);
                    self.arena.free(h);
                    out.push(Event::Filled { id: maker });
                } else {
                    level.reduce(&mut self.arena, h, fill);
                }
                if remaining.is_zero() {
                    break;
                }
            }
            if level.is_empty() {
                entry.remove();
            }
        }
        remaining
    }

    /// Checks the book's internal structure: invariants I4–I8.
    ///
    /// Costs O(resting orders); intended for tests and fuzzing.
    ///
    /// # Errors
    ///
    /// Returns the first violation found.
    pub fn check_structure(&self) -> Result<(), Violation> {
        let mut linked = 0usize;
        for (side, levels) in [(Side::Buy, &self.bids), (Side::Sell, &self.asks)] {
            for (&price, level) in levels {
                let at = || format!("{side} level {price}");
                let handles = level
                    .walk_checked(&self.arena, self.index.len())
                    .map_err(|why| Violation::new("I4", format!("{}: {why}", at())))?;
                if handles.is_empty() {
                    return Err(Violation::new("I4", format!("{}: empty level kept", at())));
                }
                let mut total = 0u128;
                let mut prev_seq: Option<Seq> = None;
                for &h in &handles {
                    let n = self.arena.get(h);
                    if n.side != side || n.price != price {
                        return Err(Violation::new(
                            "I5",
                            format!("{}: order {} records {} {}", at(), n.id, n.side, n.price),
                        ));
                    }
                    if self.index.get(&n.id) != Some(&h) {
                        return Err(Violation::new(
                            "I5",
                            format!("{}: order {} not indexed at its slot", at(), n.id),
                        ));
                    }
                    if n.open.is_zero() {
                        return Err(Violation::new(
                            "I6",
                            format!("{}: order {} has zero open", at(), n.id),
                        ));
                    }
                    if prev_seq.is_some_and(|p| p >= n.seq) {
                        return Err(Violation::new(
                            "I7",
                            format!("{}: order {} seq {} out of FIFO order", at(), n.id, n.seq.0),
                        ));
                    }
                    prev_seq = Some(n.seq);
                    total = total.saturating_add(u128::from(n.open.0));
                }
                if total != u128::from(level.total().0) || handles.len() != level.count() as usize {
                    return Err(Violation::new(
                        "I4",
                        format!(
                            "{}: cached total/count {}/{} but orders sum to {total}/{}",
                            at(),
                            level.total(),
                            level.count(),
                            handles.len()
                        ),
                    ));
                }
                linked = linked.saturating_add(handles.len());
            }
        }
        if linked != self.index.len() {
            return Err(Violation::new(
                "I5",
                format!(
                    "{} orders indexed but {linked} linked into levels",
                    self.index.len()
                ),
            ));
        }
        if let (Some(bid), Some(ask)) = (self.best_bid(), self.best_ask())
            && bid >= ask
        {
            return Err(Violation::new(
                "I8",
                format!("crossed: bid {bid} >= ask {ask}"),
            ));
        }
        Ok(())
    }

    fn levels(&self, side: Side) -> &BTreeMap<Price, Level> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(orders: &[(Side, i64, u64)]) -> OrderBook {
        let mut b = OrderBook::new();
        for (i, &(side, price, qty)) in (1u64..).zip(orders) {
            b.insert(OrderId(i), side, Price(price), Qty(qty), Seq(i));
        }
        b
    }

    fn summary(price: i64, qty: u64, orders: u32) -> LevelSummary {
        LevelSummary {
            price: Price(price),
            qty: Qty(qty),
            orders,
        }
    }

    #[test]
    fn empty_book_has_no_top() {
        let b = OrderBook::new();
        assert!(b.is_empty());
        assert_eq!((b.best_bid(), b.best_ask()), (None, None));
        assert!(b.depth(Side::Buy, 10).is_empty());
    }

    #[test]
    fn best_prices_track_the_inside() {
        let b = book(&[
            (Side::Buy, 99, 1),
            (Side::Buy, 100, 1),
            (Side::Buy, 98, 1),
            (Side::Sell, 102, 1),
            (Side::Sell, 101, 1),
            (Side::Sell, 103, 1),
        ]);
        assert_eq!(b.best_bid(), Some(Price(100)));
        assert_eq!(b.best_ask(), Some(Price(101)));
        assert_eq!(b.best(Side::Buy), b.best_bid());
        assert_eq!(b.best(Side::Sell), b.best_ask());
        assert_eq!(b.len(), 6);
    }

    #[test]
    fn depth_is_best_first_and_aggregated() {
        let b = book(&[
            (Side::Buy, 99, 5),
            (Side::Buy, 100, 2),
            (Side::Buy, 99, 3),
            (Side::Sell, 105, 4),
            (Side::Sell, 104, 1),
            (Side::Sell, 104, 6),
        ]);
        assert_eq!(
            b.depth(Side::Buy, 10),
            [summary(100, 2, 1), summary(99, 8, 2)]
        );
        assert_eq!(
            b.depth(Side::Sell, 10),
            [summary(104, 7, 2), summary(105, 4, 1)]
        );
        assert_eq!(b.depth(Side::Sell, 1), [summary(104, 7, 2)]);
    }

    #[test]
    fn level_orders_are_in_time_priority() {
        let b = book(&[
            (Side::Sell, 50, 3),
            (Side::Sell, 51, 1),
            (Side::Sell, 50, 4),
        ]);
        let ids: Vec<_> = b
            .level_orders(Side::Sell, Price(50))
            .iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, [OrderId(1), OrderId(3)]);
        assert!(b.level_orders(Side::Buy, Price(50)).is_empty());
    }

    #[test]
    fn remove_updates_level_and_drops_empty_levels() {
        let mut b = book(&[(Side::Buy, 10, 3), (Side::Buy, 10, 4), (Side::Buy, 9, 5)]);
        let removed = b.remove(OrderId(1)).unwrap();
        assert_eq!((removed.price, removed.qty), (Price(10), Qty(3)));
        assert_eq!(
            b.depth(Side::Buy, 10),
            [summary(10, 4, 1), summary(9, 5, 1)]
        );

        b.remove(OrderId(2)).unwrap();
        assert_eq!(b.best_bid(), Some(Price(9)));
        assert_eq!(b.depth(Side::Buy, 10), [summary(9, 5, 1)]);
        assert_eq!(b.order(OrderId(2)), None);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn removing_unknown_or_removed_id_is_none() {
        let mut b = book(&[(Side::Sell, 10, 1)]);
        assert_eq!(b.remove(OrderId(99)), None);
        assert!(b.remove(OrderId(1)).is_some());
        assert_eq!(b.remove(OrderId(1)), None);
        assert!(b.is_empty());
    }

    #[test]
    fn order_lookup_reports_resting_state() {
        let b = book(&[(Side::Buy, 7, 11)]);
        let o = b.order(OrderId(1)).unwrap();
        assert_eq!(
            o,
            RestingOrder {
                id: OrderId(1),
                side: Side::Buy,
                price: Price(7),
                qty: Qty(11),
                seq: Seq(1),
            }
        );
    }

    #[test]
    fn reduce_keeps_queue_position() {
        let mut b = book(&[(Side::Sell, 10, 5), (Side::Sell, 10, 5)]);
        b.reduce(OrderId(1), Qty(2));
        assert_eq!(b.depth(Side::Sell, 1), [summary(10, 7, 2)]);
        let queue: Vec<_> = b
            .level_orders(Side::Sell, Price(10))
            .iter()
            .map(|o| (o.id, o.qty))
            .collect();
        assert_eq!(queue, [(OrderId(1), Qty(2)), (OrderId(2), Qty(5))]);
    }

    mod structure {
        use super::*;

        fn sample() -> OrderBook {
            book(&[
                (Side::Buy, 99, 5),
                (Side::Buy, 99, 3),
                (Side::Buy, 98, 1),
                (Side::Sell, 101, 4),
            ])
        }

        fn violated(b: &OrderBook) -> &'static str {
            b.check_structure().unwrap_err().invariant
        }

        #[test]
        fn healthy_book_passes() {
            assert_eq!(sample().check_structure(), Ok(()));
            assert_eq!(OrderBook::new().check_structure(), Ok(()));
        }

        #[test]
        fn stale_level_total_is_i4() {
            let mut b = sample();
            b.bids.get_mut(&Price(99)).unwrap().corrupt_total(Qty(9));
            assert_eq!(violated(&b), "I4");
        }

        #[test]
        fn broken_tail_link_is_i4() {
            let mut b = sample();
            b.bids.get_mut(&Price(99)).unwrap().corrupt_tail(None);
            assert_eq!(violated(&b), "I4");
        }

        #[test]
        fn unindexed_order_is_i5() {
            let mut b = sample();
            b.index.remove(&OrderId(2));
            assert_eq!(violated(&b), "I5");
        }

        #[test]
        fn order_recording_wrong_price_is_i5() {
            let mut b = sample();
            let h = b.index[&OrderId(3)];
            b.arena.get_mut(h).price = Price(97);
            assert_eq!(violated(&b), "I5");
        }

        #[test]
        fn zero_open_order_is_i6() {
            let mut b = sample();
            let h = b.index[&OrderId(3)];
            b.arena.get_mut(h).open = Qty::ZERO;
            assert_eq!(violated(&b), "I6");
        }

        #[test]
        fn fifo_sequence_inversion_is_i7() {
            let mut b = sample();
            let h = b.index[&OrderId(2)];
            b.arena.get_mut(h).seq = Seq(1);
            assert_eq!(violated(&b), "I7");
        }

        #[test]
        fn crossed_book_is_i8() {
            let mut b = sample();
            b.insert(OrderId(9), Side::Sell, Price(99), Qty(1), Seq(9));
            assert_eq!(violated(&b), "I8");
        }
    }

    #[test]
    #[should_panic(expected = "inserted twice")]
    fn duplicate_insert_is_a_bug() {
        let mut b = book(&[(Side::Buy, 7, 1)]);
        b.insert(OrderId(1), Side::Sell, Price(8), Qty(1), Seq(2));
    }
}
