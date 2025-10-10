//! A deliberately naive order book used as a test oracle.
//!
//! Resting orders are a flat `Vec`; every match scans it for the best
//! eligible order. It shares no code with the engine beyond the public
//! types, so agreement between the two is meaningful evidence that both
//! implement the rules in `docs/requirements.md`.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "test oracle; quantities are bounded by MAX_QTY and never overflow u64"
)]

use lob::{
    CancelReason, Command, Event, MAX_PRICE, MAX_QTY, OrderId, OrderType, Price, Priority, Qty,
    RejectReason, RestingOrder, Seq, Side, Snapshot,
};

#[derive(Clone, Copy)]
struct Order {
    id: u64,
    side: Side,
    price: i64,
    open: u64,
    seq: u64,
}

pub(crate) struct Model {
    orders: Vec<Order>,
    next_id: u64,
    next_seq: u64,
}

impl Model {
    pub(crate) fn new() -> Self {
        Self {
            orders: Vec::new(),
            next_id: 1,
            next_seq: 1,
        }
    }

    pub(crate) fn apply(&mut self, cmd: Command) -> Vec<Event> {
        let mut out = Vec::new();
        match cmd {
            Command::Submit { side, kind, qty } => self.submit(side, kind, qty.0, &mut out),
            Command::Cancel { id } => self.cancel(id.0, &mut out),
            Command::Amend { id, price, qty } => {
                self.amend(id.0, price.map(|p| p.0), qty.0, &mut out);
            }
        }
        out
    }

    fn reject(id: Option<u64>, reason: RejectReason, out: &mut Vec<Event>) {
        out.push(Event::Rejected {
            id: id.map(OrderId),
            reason,
        });
    }

    fn field_error(qty: u64, price: Option<i64>) -> Option<RejectReason> {
        if qty == 0 {
            Some(RejectReason::ZeroQuantity)
        } else if qty > MAX_QTY {
            Some(RejectReason::QuantityTooLarge)
        } else if price.is_some_and(|p| !(1..=MAX_PRICE).contains(&p)) {
            Some(RejectReason::PriceOutOfRange)
        } else {
            None
        }
    }

    fn submit(&mut self, side: Side, kind: OrderType, qty: u64, out: &mut Vec<Event>) {
        let limit = match kind {
            OrderType::Limit { price } | OrderType::Ioc { price } => Some(price.0),
            OrderType::Market => None,
        };
        if let Some(reason) = Self::field_error(qty, limit) {
            return Self::reject(None, reason, out);
        }
        let id = self.next_id;
        let seq = self.next_seq;
        self.next_id += 1;
        self.next_seq += 1;
        out.push(Event::Accepted {
            id: OrderId(id),
            side,
            kind,
            qty: Qty(qty),
            seq: Seq(seq),
        });
        let left = self.take_liquidity(id, side, limit, qty, out);
        if left == 0 {
            out.push(Event::Filled { id: OrderId(id) });
            return;
        }
        match kind {
            OrderType::Limit { price } => self.rest(id, side, price.0, left, seq, out),
            OrderType::Ioc { .. } => out.push(Event::Cancelled {
                id: OrderId(id),
                qty: Qty(left),
                reason: CancelReason::IocRemainder,
            }),
            OrderType::Market => out.push(Event::Cancelled {
                id: OrderId(id),
                qty: Qty(left),
                reason: CancelReason::NoLiquidity,
            }),
        }
    }

    fn cancel(&mut self, id: u64, out: &mut Vec<Event>) {
        match self.orders.iter().position(|o| o.id == id) {
            Some(i) => {
                let o = self.orders.remove(i);
                out.push(Event::Cancelled {
                    id: OrderId(id),
                    qty: Qty(o.open),
                    reason: CancelReason::User,
                });
            }
            None => Self::reject(Some(id), RejectReason::UnknownOrder, out),
        }
    }

    fn amend(&mut self, id: u64, price: Option<i64>, qty: u64, out: &mut Vec<Event>) {
        if let Some(reason) = Self::field_error(qty, price) {
            return Self::reject(Some(id), reason, out);
        }
        let Some(i) = self.orders.iter().position(|o| o.id == id) else {
            return Self::reject(Some(id), RejectReason::UnknownOrder, out);
        };
        let cur = self.orders[i];
        let new_price = price.unwrap_or(cur.price);
        if new_price == cur.price && qty == cur.open {
            return Self::reject(Some(id), RejectReason::AmendNoChange, out);
        }
        if new_price == cur.price && qty < cur.open {
            self.orders[i].open = qty;
            out.push(Event::Amended {
                id: OrderId(id),
                price: Price(new_price),
                old_qty: Qty(cur.open),
                qty: Qty(qty),
                seq: Seq(cur.seq),
                priority: Priority::Kept,
            });
            return;
        }
        self.orders.remove(i);
        let seq = self.next_seq;
        self.next_seq += 1;
        out.push(Event::Amended {
            id: OrderId(id),
            price: Price(new_price),
            old_qty: Qty(cur.open),
            qty: Qty(qty),
            seq: Seq(seq),
            priority: Priority::Lost,
        });
        let left = self.take_liquidity(id, cur.side, Some(new_price), qty, out);
        if left == 0 {
            out.push(Event::Filled { id: OrderId(id) });
        } else {
            self.rest(id, cur.side, new_price, left, seq, out);
        }
    }

    fn rest(&mut self, id: u64, side: Side, price: i64, open: u64, seq: u64, out: &mut Vec<Event>) {
        self.orders.push(Order {
            id,
            side,
            price,
            open,
            seq,
        });
        out.push(Event::Rested {
            id: OrderId(id),
            side,
            price: Price(price),
            qty: Qty(open),
            seq: Seq(seq),
        });
    }

    /// Repeatedly picks the best eligible resting order by scanning them all.
    fn take_liquidity(
        &mut self,
        taker: u64,
        side: Side,
        limit: Option<i64>,
        mut left: u64,
        out: &mut Vec<Event>,
    ) -> u64 {
        while left > 0 {
            let eligible = self.orders.iter().enumerate().filter(|(_, o)| {
                o.side != side
                    && limit.is_none_or(|l| match side {
                        Side::Buy => o.price <= l,
                        Side::Sell => o.price >= l,
                    })
            });
            let best = match side {
                Side::Buy => eligible.min_by_key(|(_, o)| (o.price, o.seq)),
                Side::Sell => eligible.min_by_key(|(_, o)| (-o.price, o.seq)),
            };
            let Some((i, _)) = best else { break };
            let maker = &mut self.orders[i];
            let fill = left.min(maker.open);
            out.push(Event::Trade {
                maker: OrderId(maker.id),
                taker: OrderId(taker),
                taker_side: side,
                price: Price(maker.price),
                qty: Qty(fill),
            });
            left -= fill;
            maker.open -= fill;
            if maker.open == 0 {
                let id = maker.id;
                self.orders.remove(i);
                out.push(Event::Filled { id: OrderId(id) });
            }
        }
        left
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let view = |o: &Order| RestingOrder {
            id: OrderId(o.id),
            side: o.side,
            price: Price(o.price),
            qty: Qty(o.open),
            seq: Seq(o.seq),
        };
        let mut bids: Vec<_> = self.orders.iter().filter(|o| o.side == Side::Buy).collect();
        let mut asks: Vec<_> = self
            .orders
            .iter()
            .filter(|o| o.side == Side::Sell)
            .collect();
        bids.sort_by_key(|o| (-o.price, o.seq));
        asks.sort_by_key(|o| (o.price, o.seq));
        Snapshot {
            next_id: self.next_id,
            next_seq: self.next_seq,
            bids: bids.into_iter().map(view).collect(),
            asks: asks.into_iter().map(view).collect(),
        }
    }
}
