//! Matching behaviour through the public API, grouped by user story.

use lob::{
    CancelReason, Event, LevelSummary, MatchingEngine, OrderId, OrderType, Price, Qty,
    RejectReason, Seq, Side,
};

use Side::{Buy, Sell};

struct Harness {
    engine: MatchingEngine,
}

impl Harness {
    fn new() -> Self {
        Self {
            engine: MatchingEngine::new(),
        }
    }

    fn submit(&mut self, side: Side, kind: OrderType, qty: u64) -> Vec<Event> {
        let mut out = Vec::new();
        self.engine.submit(side, kind, Qty(qty), &mut out);
        out
    }

    fn limit(&mut self, side: Side, price: i64, qty: u64) -> Vec<Event> {
        self.submit(side, limit(price), qty)
    }

    fn cancel(&mut self, id: u64) -> Vec<Event> {
        let mut out = Vec::new();
        self.engine.cancel(OrderId(id), &mut out);
        out
    }

    fn depth(&self, side: Side) -> Vec<(i64, u64, u32)> {
        self.engine
            .book()
            .depth(side, usize::MAX)
            .into_iter()
            .map(|LevelSummary { price, qty, orders }| (price.0, qty.0, orders))
            .collect()
    }

    fn queue(&self, side: Side, price: i64) -> Vec<(u64, u64)> {
        self.engine
            .book()
            .level_orders(side, Price(price))
            .into_iter()
            .map(|o| (o.id.0, o.qty.0))
            .collect()
    }
}

fn limit(price: i64) -> OrderType {
    OrderType::Limit {
        price: Price(price),
    }
}

fn ioc(price: i64) -> OrderType {
    OrderType::Ioc {
        price: Price(price),
    }
}

fn trades(events: &[Event]) -> Vec<(u64, u64, i64, u64)> {
    events
        .iter()
        .filter_map(|e| match *e {
            Event::Trade {
                maker,
                taker,
                price,
                qty,
                ..
            } => Some((maker.0, taker.0, price.0, qty.0)),
            _ => None,
        })
        .collect()
}

fn last(events: &[Event]) -> Event {
    *events.last().expect("at least one event")
}

// US-001 — submit limit order

#[test]
fn non_crossing_limit_rests_with_new_id() {
    let mut h = Harness::new();
    let ev = h.limit(Buy, 100, 5);
    assert_eq!(
        ev,
        [
            Event::Accepted {
                id: OrderId(1),
                side: Buy,
                kind: limit(100),
                qty: Qty(5),
                seq: Seq(1),
            },
            Event::Rested {
                id: OrderId(1),
                side: Buy,
                price: Price(100),
                qty: Qty(5),
                seq: Seq(1),
            },
        ]
    );
    assert_eq!(h.depth(Buy), [(100, 5, 1)]);
    assert_eq!(h.engine.book().best_bid(), Some(Price(100)));
}

#[test]
fn ids_increase_monotonically_and_rejections_consume_none() {
    let mut h = Harness::new();
    h.limit(Buy, 100, 1);
    h.limit(Buy, 100, 0);
    let ev = h.limit(Sell, 105, 1);
    assert!(matches!(
        ev[0],
        Event::Accepted {
            id: OrderId(2),
            seq: Seq(2),
            ..
        }
    ));
}

// US-002 — trade at the best available price

#[test]
fn crossing_buy_trades_at_lowest_ask_first_at_maker_price() {
    let mut h = Harness::new();
    h.limit(Sell, 103, 5); // 1
    h.limit(Sell, 101, 5); // 2
    h.limit(Sell, 102, 5); // 3
    let ev = h.limit(Buy, 110, 5);
    assert_eq!(trades(&ev), [(2, 4, 101, 5)]);
    assert_eq!(h.depth(Sell), [(102, 5, 1), (103, 5, 1)]);
}

#[test]
fn crossing_sell_trades_at_highest_bid_first() {
    let mut h = Harness::new();
    h.limit(Buy, 98, 5);
    h.limit(Buy, 99, 5);
    let ev = h.limit(Sell, 90, 3);
    assert_eq!(trades(&ev), [(2, 3, 99, 3)]);
}

#[test]
fn limit_never_trades_through_its_price() {
    let mut h = Harness::new();
    h.limit(Sell, 101, 5);
    h.limit(Sell, 102, 5);
    let ev = h.limit(Buy, 101, 8);
    assert_eq!(trades(&ev), [(1, 3, 101, 5)]);
    assert_eq!(h.depth(Buy), [(101, 3, 1)]);
    assert_eq!(h.depth(Sell), [(102, 5, 1)]);
}

// US-003 — partial fills across levels

#[test]
fn sweep_fills_level_by_level_and_rests_remainder() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 2);
    h.limit(Sell, 101, 3);
    h.limit(Sell, 102, 4);
    let ev = h.limit(Buy, 101, 10);
    assert_eq!(trades(&ev), [(1, 4, 100, 2), (2, 4, 101, 3)]);
    assert_eq!(
        last(&ev),
        Event::Rested {
            id: OrderId(4),
            side: Buy,
            price: Price(101),
            qty: Qty(5),
            seq: Seq(4),
        }
    );
    assert_eq!(h.depth(Buy), [(101, 5, 1)]);
    assert_eq!(h.depth(Sell), [(102, 4, 1)]);
}

#[test]
fn partially_filled_maker_keeps_head_of_queue() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 10); // 1
    h.limit(Sell, 100, 10); // 2
    h.limit(Buy, 100, 4);
    assert_eq!(h.queue(Sell, 100), [(1, 6), (2, 10)]);
    let ev = h.limit(Buy, 100, 7);
    assert_eq!(trades(&ev), [(1, 4, 100, 6), (2, 4, 100, 1)]);
    assert_eq!(h.queue(Sell, 100), [(2, 9)]);
}

#[test]
fn filled_events_follow_each_completed_maker_and_the_taker() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 2);
    h.limit(Sell, 100, 3);
    let ev = h.limit(Buy, 100, 5);
    let fills: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::Filled { id } => Some(id.0),
            _ => None,
        })
        .collect();
    assert_eq!(fills, [1, 2, 3]);
    assert!(h.engine.book().is_empty());
}

// US-004 — time priority within a level

#[test]
fn same_price_orders_fill_in_arrival_order() {
    let mut h = Harness::new();
    h.limit(Buy, 50, 1); // 1
    h.limit(Buy, 50, 1); // 2
    h.limit(Buy, 51, 1); // 3: better price, arrives later
    h.limit(Buy, 50, 1); // 4
    let ev = h.limit(Sell, 50, 4);
    let makers: Vec<_> = trades(&ev).iter().map(|t| t.0).collect();
    assert_eq!(makers, [3, 1, 2, 4]);
}

// US-005 — market order

#[test]
fn market_order_sweeps_all_prices() {
    let mut h = Harness::new();
    h.limit(Buy, 10, 1);
    h.limit(Buy, 5, 1);
    h.limit(Buy, 1, 1);
    let ev = h.submit(Sell, OrderType::Market, 3);
    assert_eq!(trades(&ev), [(1, 4, 10, 1), (2, 4, 5, 1), (3, 4, 1, 1)]);
    assert_eq!(last(&ev), Event::Filled { id: OrderId(4) });
}

#[test]
fn market_remainder_is_cancelled_and_never_rests() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 2);
    let ev = h.submit(Buy, OrderType::Market, 5);
    assert_eq!(trades(&ev), [(1, 2, 100, 2)]);
    assert_eq!(
        last(&ev),
        Event::Cancelled {
            id: OrderId(2),
            qty: Qty(3),
            reason: CancelReason::NoLiquidity,
        }
    );
    assert!(h.engine.book().is_empty());
}

#[test]
fn market_order_on_empty_book_is_cancelled_whole() {
    let mut h = Harness::new();
    let ev = h.submit(Buy, OrderType::Market, 7);
    assert_eq!(ev.len(), 2);
    assert_eq!(
        last(&ev),
        Event::Cancelled {
            id: OrderId(1),
            qty: Qty(7),
            reason: CancelReason::NoLiquidity,
        }
    );
}

// US-006 — immediate-or-cancel

#[test]
fn ioc_trades_within_limit_then_cancels_remainder() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 2);
    h.limit(Sell, 101, 2);
    let ev = h.submit(Buy, ioc(100), 5);
    assert_eq!(trades(&ev), [(1, 3, 100, 2)]);
    assert_eq!(
        last(&ev),
        Event::Cancelled {
            id: OrderId(3),
            qty: Qty(3),
            reason: CancelReason::IocRemainder,
        }
    );
    assert!(h.depth(Buy).is_empty());
    assert_eq!(h.depth(Sell), [(101, 2, 1)]);
}

#[test]
fn fully_filled_ioc_reports_filled_not_cancelled() {
    let mut h = Harness::new();
    h.limit(Buy, 100, 5);
    let ev = h.submit(Sell, ioc(99), 5);
    assert_eq!(last(&ev), Event::Filled { id: OrderId(2) });
}

// US-007 — cancel

#[test]
fn cancel_removes_order_and_reports_open_quantity() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 10);
    h.limit(Sell, 100, 5);
    h.limit(Buy, 100, 4);
    let ev = h.cancel(1);
    assert_eq!(
        ev,
        [Event::Cancelled {
            id: OrderId(1),
            qty: Qty(6),
            reason: CancelReason::User,
        }]
    );
    assert_eq!(h.queue(Sell, 100), [(2, 5)]);
}

#[test]
fn cancel_of_unknown_filled_or_cancelled_order_is_rejected() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 1); // 1
    h.limit(Buy, 100, 1); // 2, fills 1
    h.limit(Buy, 90, 1); // 3
    h.cancel(3);
    for id in [0, 1, 2, 3, 99] {
        assert_eq!(
            h.cancel(id),
            [Event::Rejected {
                id: Some(OrderId(id)),
                reason: RejectReason::UnknownOrder,
            }],
            "cancel {id}"
        );
    }
}

// US-009 — invalid submissions

#[test]
fn invalid_submissions_are_rejected_without_side_effects() {
    let mut h = Harness::new();
    h.limit(Sell, 100, 5);
    let cases = [
        (limit(100), 0, RejectReason::ZeroQuantity),
        (OrderType::Market, u64::MAX, RejectReason::QuantityTooLarge),
        (limit(0), 1, RejectReason::PriceOutOfRange),
        (ioc(-1), 1, RejectReason::PriceOutOfRange),
    ];
    for (kind, qty, reason) in cases {
        let ev = h.submit(Buy, kind, qty);
        assert_eq!(ev, [Event::Rejected { id: None, reason }]);
    }
    assert_eq!(h.depth(Sell), [(100, 5, 1)]);
    assert!(h.depth(Buy).is_empty());
}
