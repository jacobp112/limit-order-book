//! Runtime checks of the invariants in `docs/requirements.md`.
//!
//! Two independent views are compared:
//!
//! - [`OrderBook::check_structure`](crate::OrderBook::check_structure)
//!   inspects the book's internals (I4–I8).
//! - [`Auditor`] rebuilds per-order accounting purely from commands and
//!   events, then reconciles it with what the book reports (I1–I3, I9, I10).
//!
//! The auditor never reads engine internals, so a bookkeeping bug in the
//! engine cannot also hide itself from the check.

use std::collections::BTreeMap;
use std::fmt;

use crate::book::OrderBook;
use crate::command::{Command, Event};
use crate::snapshot::Snapshot;
use crate::types::{OrderId, Price, Qty, Seq, Side};

/// A broken invariant, named as in `docs/requirements.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Invariant label, e.g. `"I1"`.
    pub invariant: &'static str,
    /// What was observed.
    pub detail: String,
}

impl Violation {
    pub(crate) fn new(invariant: &'static str, detail: impl Into<String>) -> Self {
        Self {
            invariant,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.invariant, self.detail)
    }
}

impl std::error::Error for Violation {}

/// Per-order accounting rebuilt from events.
///
/// Quantities are `u128`: order quantities are at most 10^9, so no realistic
/// run can overflow, and [`add`] panics rather than wraps if one ever did.
#[derive(Clone, Debug)]
struct Entry {
    side: Side,
    /// Limit price when this order is the taker; `None` for market orders.
    limit: Option<Price>,
    /// Where the order rests, if it does: price and priority.
    resting: Option<(Price, Seq)>,
    accepted: u128,
    filled: u128,
    cancelled: u128,
}

impl Entry {
    /// `A - F - C`: what should still be open. `None` if F + C > A.
    fn open(&self) -> Option<u128> {
        self.accepted
            .checked_sub(self.filled)?
            .checked_sub(self.cancelled)
    }
}

fn add(a: &mut u128, b: u128) {
    *a = a.checked_add(b).expect("u128 ledger overflow");
}

fn inc(n: &mut usize) {
    *n = n.checked_add(1).expect("resting count overflow");
}

fn dec(n: &mut usize) {
    *n = n.checked_sub(1).expect("resting count underflow");
}

fn wide(q: Qty) -> u128 {
    u128::from(q.0)
}

fn entry(orders: &mut BTreeMap<OrderId, Entry>, id: OrderId) -> Result<&mut Entry, Violation> {
    orders.get_mut(&id).ok_or_else(|| {
        Violation::new(
            "I1",
            format!("event for order {id} that was never accepted"),
        )
    })
}

/// Checks each command's events and the resulting book against I1–I10.
///
/// Feed it every command, the events it produced and the book afterwards.
#[derive(Debug, Default)]
pub struct Auditor {
    orders: BTreeMap<OrderId, Entry>,
    resting: usize,
    buy_filled: u128,
    sell_filled: u128,
    traded: u128,
}

impl Auditor {
    /// An auditor for an engine starting from an empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An auditor for an engine restored from `snapshot`. Orders already
    /// resting are treated as accepted with their current open quantity.
    #[must_use]
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        let mut a = Self::new();
        for o in snapshot.bids.iter().chain(&snapshot.asks) {
            a.orders.insert(
                o.id,
                Entry {
                    side: o.side,
                    limit: Some(o.price),
                    resting: Some((o.price, o.seq)),
                    accepted: wide(o.qty),
                    filled: 0,
                    cancelled: 0,
                },
            );
        }
        a.resting = a.orders.len();
        a
    }

    /// Total traded quantity seen so far.
    #[must_use]
    pub fn traded(&self) -> u128 {
        self.traded
    }

    /// Checks one command's events and the resulting book.
    ///
    /// # Errors
    ///
    /// Returns the first invariant found broken.
    pub fn observe(
        &mut self,
        cmd: &Command,
        events: &[Event],
        book: &OrderBook,
    ) -> Result<(), Violation> {
        self.check_events(cmd, events)?;
        book.check_structure()?;
        self.reconcile(events, book)
    }

    fn check_events(&mut self, cmd: &Command, events: &[Event]) -> Result<(), Violation> {
        match events {
            [] => return Err(Violation::new("I3", format!("`{cmd}` emitted no events"))),
            [Event::Rejected { .. }] => return Ok(()),
            _ if events.iter().any(|e| matches!(e, Event::Rejected { .. })) => {
                return Err(Violation::new(
                    "I3",
                    format!("`{cmd}` was rejected but also emitted other events"),
                ));
            }
            _ => {}
        }

        // Previous fill of the current taker, for the priority check (I10).
        let mut last_fill: Option<(Price, Seq)> = None;
        for event in events {
            match *event {
                Event::Accepted {
                    id,
                    side,
                    kind,
                    qty,
                    ..
                } => {
                    let fresh = Entry {
                        side,
                        limit: kind.limit(),
                        resting: None,
                        accepted: wide(qty),
                        filled: 0,
                        cancelled: 0,
                    };
                    if self.orders.insert(id, fresh).is_some() {
                        return Err(Violation::new("I1", format!("order {id} accepted twice")));
                    }
                    last_fill = None;
                }
                Event::Amended {
                    id,
                    price,
                    old_qty,
                    qty,
                    ..
                } => {
                    let e = entry(&mut self.orders, id)?;
                    if e.resting.is_none() || e.open() != Some(wide(old_qty)) {
                        return Err(Violation::new(
                            "I1",
                            format!(
                                "amend of {id} from {old_qty}, ledger has {:?} open",
                                e.open()
                            ),
                        ));
                    }
                    match qty.cmp(&old_qty) {
                        std::cmp::Ordering::Greater => add(
                            &mut e.accepted,
                            wide(qty.checked_sub(old_qty).expect("qty > old")),
                        ),
                        _ => add(
                            &mut e.cancelled,
                            wide(old_qty.checked_sub(qty).expect("old >= qty")),
                        ),
                    }
                    e.limit = Some(price);
                    // A priority-losing amend leaves the level; `Rested`
                    // puts it back. A kept-priority amend stays where it is.
                    if let Event::Amended {
                        priority: crate::Priority::Lost,
                        ..
                    } = *event
                    {
                        e.resting = None;
                        dec(&mut self.resting);
                    }
                    last_fill = None;
                }
                Event::Trade {
                    maker,
                    taker,
                    taker_side,
                    price,
                    qty,
                } => {
                    let q = wide(qty);
                    let m = entry(&mut self.orders, maker)?;
                    let Some((maker_price, maker_seq)) = m.resting else {
                        return Err(Violation::new(
                            "I9",
                            format!("maker {maker} was not resting"),
                        ));
                    };
                    if m.side == taker_side {
                        return Err(Violation::new(
                            "I9",
                            format!("maker {maker} is on the taker's side"),
                        ));
                    }
                    if price != maker_price {
                        return Err(Violation::new(
                            "I9",
                            format!("trade at {price} but maker {maker} rests at {maker_price}"),
                        ));
                    }
                    add(&mut m.filled, q);
                    let t = entry(&mut self.orders, taker)?;
                    if t.side != taker_side {
                        return Err(Violation::new("I9", format!("taker {taker} side mismatch")));
                    }
                    if t.limit
                        .is_some_and(|limit| !taker_side.crosses(limit, price))
                    {
                        return Err(Violation::new(
                            "I9",
                            format!(
                                "taker {taker} traded at {price} outside its limit {:?}",
                                t.limit
                            ),
                        ));
                    }
                    add(&mut t.filled, q);
                    if let Some((p, s)) = last_fill {
                        let not_worse = match taker_side {
                            Side::Buy => price >= p,
                            Side::Sell => price <= p,
                        };
                        if !not_worse || (price == p && maker_seq <= s) {
                            return Err(Violation::new(
                                "I10",
                                format!("taker {taker} filled maker {maker} at {price} after {p}"),
                            ));
                        }
                    }
                    last_fill = Some((price, maker_seq));
                    add(&mut self.buy_filled, q);
                    add(&mut self.sell_filled, q);
                    add(&mut self.traded, q);
                }
                Event::Filled { id } => {
                    let e = entry(&mut self.orders, id)?;
                    if e.open() != Some(0) {
                        return Err(Violation::new(
                            "I1",
                            format!("order {id} reported filled with {:?} open", e.open()),
                        ));
                    }
                    if e.resting.take().is_some() {
                        dec(&mut self.resting);
                    }
                }
                Event::Rested {
                    id,
                    side,
                    price,
                    qty,
                    seq,
                } => {
                    let e = entry(&mut self.orders, id)?;
                    if e.side != side || e.open() != Some(wide(qty)) || e.resting.is_some() {
                        return Err(Violation::new(
                            "I1",
                            format!("order {id} rested {qty}, ledger has {:?} open", e.open()),
                        ));
                    }
                    e.resting = Some((price, seq));
                    inc(&mut self.resting);
                }
                Event::Cancelled { id, qty, .. } => {
                    let e = entry(&mut self.orders, id)?;
                    add(&mut e.cancelled, wide(qty));
                    if e.open() != Some(0) {
                        return Err(Violation::new(
                            "I1",
                            format!("order {id} cancelled {qty} leaving {:?} open", e.open()),
                        ));
                    }
                    if e.resting.take().is_some() {
                        dec(&mut self.resting);
                    }
                }
                Event::Rejected { .. } => unreachable!("handled above"),
            }
        }
        // Each trade adds its quantity once to each side's total, so this
        // guards the ledger's own bookkeeping as much as the engine's.
        if self.buy_filled != self.traded || self.sell_filled != self.traded {
            return Err(Violation::new(
                "I2",
                format!(
                    "buy fills {} / sell fills {} / traded {}",
                    self.buy_filled, self.sell_filled, self.traded
                ),
            ));
        }
        Ok(())
    }

    /// I1 against the book, using the book's open quantity rather than the
    /// ledger's: `A = F + O + C` for every order this command touched, and
    /// the book holds exactly as many orders as the ledger says are resting.
    fn reconcile(&self, events: &[Event], book: &OrderBook) -> Result<(), Violation> {
        for id in events.iter().filter_map(touched) {
            self.balance(id, book)?;
        }
        if self.resting != book.len() {
            return Err(Violation::new(
                "I1",
                format!(
                    "ledger has {} resting orders, book has {}",
                    self.resting,
                    book.len()
                ),
            ));
        }
        Ok(())
    }

    /// Full I1 check over every order ever seen. O(orders).
    ///
    /// # Errors
    ///
    /// Returns the first order whose accounting does not balance.
    pub fn reconcile_all(&self, book: &OrderBook) -> Result<(), Violation> {
        self.orders
            .keys()
            .try_for_each(|&id| self.balance(id, book))
    }

    fn balance(&self, id: OrderId, book: &OrderBook) -> Result<(), Violation> {
        let e = &self.orders[&id];
        let resting = book.order(id);
        let open = resting.map_or(0, |o| wide(o.qty));
        let sum = e
            .filled
            .checked_add(open)
            .and_then(|s| s.checked_add(e.cancelled));
        if sum != Some(e.accepted) {
            return Err(Violation::new(
                "I1",
                format!(
                    "order {id}: accepted {} != filled {} + open {open} + cancelled {}",
                    e.accepted, e.filled, e.cancelled
                ),
            ));
        }
        let expected = e.resting;
        let actual = resting.map(|o| (o.price, o.seq));
        if expected != actual {
            return Err(Violation::new(
                "I5",
                format!("order {id}: ledger expects resting at {expected:?}, book has {actual:?}"),
            ));
        }
        Ok(())
    }
}

fn touched(e: &Event) -> Option<OrderId> {
    match *e {
        Event::Accepted { id, .. }
        | Event::Amended { id, .. }
        | Event::Filled { id }
        | Event::Rested { id, .. }
        | Event::Cancelled { id, .. } => Some(id),
        Event::Trade { maker, .. } => Some(maker),
        Event::Rejected { .. } => None,
    }
}
