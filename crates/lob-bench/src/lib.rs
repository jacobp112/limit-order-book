//! Deterministic benchmark workloads.
//!
//! A workload is a starting state plus a fixed batch of commands. Benchmarks
//! restore the state outside the timed region and time only the batch, so
//! every run measures exactly the same work. All randomness comes from a
//! fixed-seed xorshift generator.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "workload construction with small bounded values, not engine accounting"
)]

use lob::{Command, MatchingEngine, OrderId, OrderType, Price, Qty, Side, Snapshot};

/// Price around which every workload's book is built.
pub const MID: i64 = 10_000;

/// A starting state and the commands to time against it.
#[derive(Debug, Clone)]
pub struct Workload {
    /// Short identifier used in benchmark and report names.
    pub name: &'static str,
    /// What the batch does, for reports.
    pub description: &'static str,
    /// State restored before each timed batch.
    pub start: Snapshot,
    /// The timed commands.
    pub commands: Vec<Command>,
}

impl Workload {
    /// A fresh engine in the starting state.
    ///
    /// # Panics
    ///
    /// Never for workloads built by this crate; their states are valid.
    #[must_use]
    pub fn engine(&self) -> MatchingEngine {
        MatchingEngine::restore(&self.start).expect("workload state is valid")
    }
}

/// Deterministic xorshift64 generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator; a zero seed is replaced with 1.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Value in `0..n` for `n > 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

fn limit(side: Side, price: i64, qty: u64) -> Command {
    Command::Submit {
        side,
        kind: OrderType::Limit {
            price: Price(price),
        },
        qty: Qty(qty),
    }
}

fn state_after(commands: impl IntoIterator<Item = Command>) -> Snapshot {
    let mut engine = MatchingEngine::new();
    engine.replay(commands);
    engine.snapshot()
}

/// `levels` price levels on each side of `MID`, `per_level` orders of `qty`
/// each. Bids get ids first, then asks.
fn deep_book(levels: i64, per_level: u64, qty: u64) -> Vec<Command> {
    let mut cmds = Vec::new();
    for (side, sign) in [(Side::Buy, -1), (Side::Sell, 1)] {
        for level in 1..=levels {
            for _ in 0..per_level {
                cmds.push(limit(side, MID + sign * level, qty));
            }
        }
    }
    cmds
}

/// Distinct ids from `1..=max`, in random order.
fn distinct_ids(rng: &mut Rng, max: u64, n: usize) -> Vec<OrderId> {
    let mut ids: Vec<u64> = (1..=max).collect();
    for i in 0..n {
        let j = i + rng.below((ids.len() - i) as u64) as usize;
        ids.swap(i, j);
    }
    ids.truncate(n);
    ids.into_iter().map(OrderId).collect()
}

/// 1,000 non-crossing limit orders joining existing levels of a
/// 100-level × 10-order book.
#[must_use]
pub fn rest_insert() -> Workload {
    let mut rng = Rng::new(11);
    let commands = (0..1_000)
        .map(|_| {
            let offset = 1 + rng.below(100) as i64;
            if rng.below(2) == 0 {
                limit(Side::Buy, MID - offset, 5)
            } else {
                limit(Side::Sell, MID + offset, 5)
            }
        })
        .collect();
    Workload {
        name: "rest_insert",
        description: "non-crossing limit joins an existing level (100 levels x 10 orders per side)",
        start: state_after(deep_book(100, 10, 10)),
        commands,
    }
}

/// 1,000 cancels of distinct random orders, mostly mid-queue, in a book of
/// 20,000 resting orders.
#[must_use]
pub fn cancel() -> Workload {
    let mut rng = Rng::new(12);
    let commands = distinct_ids(&mut rng, 20_000, 1_000)
        .into_iter()
        .map(|id| Command::Cancel { id })
        .collect();
    Workload {
        name: "cancel",
        description: "cancel a random resting order (20,000 resting, 100 per level)",
        start: state_after(deep_book(100, 100, 10)),
        commands,
    }
}

/// 1,000 amends of distinct random orders: half same-price decreases (keep
/// priority), half moves to another non-crossing level (lose priority).
#[must_use]
pub fn amend() -> Workload {
    let mut rng = Rng::new(13);
    let commands = distinct_ids(&mut rng, 20_000, 1_000)
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            if i % 2 == 0 {
                Command::Amend {
                    id,
                    price: None,
                    qty: Qty(5),
                }
            } else {
                // In `deep_book(100, 100, _)` ids 1..=10,000 are bids and the
                // rest asks, 100 per level; move to a *different* level so the
                // amend is never a no-op.
                let current = ((id.0 - 1) % 10_000 / 100) as i64; // level - 1
                let offset = 1 + (current + 1 + rng.below(99) as i64) % 100;
                let price = if id.0 <= 10_000 {
                    MID - offset
                } else {
                    MID + offset
                };
                Command::Amend {
                    id,
                    price: Some(Price(price)),
                    qty: Qty(10),
                }
            }
        })
        .collect();
    Workload {
        name: "amend",
        description: "amend a random order: 50% size-down in place, 50% re-price (20,000 resting)",
        start: state_after(deep_book(100, 100, 10)),
        commands,
    }
}

/// 1,000 crossing limit buys, each filling exactly one resting ask.
#[must_use]
pub fn cross() -> Workload {
    let setup =
        (1..=100).flat_map(|level| (0..100).map(move |_| limit(Side::Sell, MID + level, 1)));
    Workload {
        name: "cross",
        description: "crossing limit fully fills exactly one maker (10,000 one-lot asks)",
        start: state_after(setup),
        commands: (0..1_000).map(|_| limit(Side::Buy, MID + 100, 1)).collect(),
    }
}

/// 1,000 small takers each partially filling the same large maker.
#[must_use]
pub fn partial_fill() -> Workload {
    Workload {
        name: "partial_fill",
        description: "crossing limit partially fills a large maker at the head of the queue",
        start: state_after((0..10).map(|_| limit(Side::Sell, MID + 1, 1_000_000))),
        commands: (0..1_000).map(|_| limit(Side::Buy, MID + 1, 7)).collect(),
    }
}

/// 100 market buys, each sweeping 10 price levels (50 fills).
#[must_use]
pub fn sweep() -> Workload {
    let setup =
        (1..=1_000).flat_map(|level| (0..5).map(move |_| limit(Side::Sell, MID + level, 2)));
    Workload {
        name: "sweep",
        description: "market order sweeps 10 levels x 5 orders (50 fills)",
        start: state_after(setup),
        commands: (0..100)
            .map(|_| Command::Submit {
                side: Side::Buy,
                kind: OrderType::Market,
                qty: Qty(100),
            })
            .collect(),
    }
}

/// Mixed flow: 55% limit, 8% IOC, 5% market, 16% cancel, 16% amend, with
/// prices near `MID`. The state is warmed with 20,000 commands first.
#[must_use]
pub fn mixed() -> Workload {
    let stream = mixed_stream(21, 30_000);
    let (warm, timed) = stream.split_at(20_000);
    Workload {
        name: "mixed",
        description: "synthetic mix of limit/IOC/market/cancel/amend after 20,000-command warm-up",
        start: state_after(warm.iter().copied()),
        commands: timed.to_vec(),
    }
}

/// Deterministic mixed order flow around `MID`.
#[must_use]
pub fn mixed_stream(seed: u64, len: usize) -> Vec<Command> {
    let mut rng = Rng::new(seed);
    let mut issued = 0u64;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        let side = if rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let offset = rng.below(20) as i64 - 4;
        let price = Price(match side {
            Side::Buy => MID - offset,
            Side::Sell => MID + offset,
        });
        let qty = Qty(1 + rng.below(20));
        let recent = OrderId(issued.saturating_sub(rng.below(issued.clamp(1, 200))));
        let cmd = match rng.below(100) {
            0..=54 => Command::Submit {
                side,
                kind: OrderType::Limit { price },
                qty,
            },
            55..=62 => Command::Submit {
                side,
                kind: OrderType::Ioc { price },
                qty,
            },
            63..=67 => Command::Submit {
                side,
                kind: OrderType::Market,
                qty,
            },
            68..=83 => Command::Cancel { id: recent },
            _ => Command::Amend {
                id: recent,
                price: (rng.below(2) == 0).then_some(price),
                qty,
            },
        };
        if matches!(cmd, Command::Submit { .. }) {
            issued += 1;
        }
        out.push(cmd);
    }
    out
}

/// Every workload, in report order.
#[must_use]
pub fn all() -> Vec<Workload> {
    vec![
        rest_insert(),
        cross(),
        cancel(),
        amend(),
        partial_fill(),
        sweep(),
        mixed(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob::{Event, Priority};

    fn run(w: &Workload) -> Vec<Event> {
        let mut engine = w.engine();
        engine.replay(w.commands.iter().copied())
    }

    fn count(events: &[Event], pred: impl Fn(&Event) -> bool) -> usize {
        events.iter().filter(|e| pred(e)).count()
    }

    fn rejected(e: &Event) -> bool {
        matches!(e, Event::Rejected { .. })
    }

    #[test]
    fn rest_insert_only_rests() {
        let ev = run(&rest_insert());
        assert_eq!(count(&ev, |e| matches!(e, Event::Rested { .. })), 1_000);
        assert_eq!(count(&ev, |e| matches!(e, Event::Trade { .. })), 0);
    }

    #[test]
    fn cancel_cancels_every_target() {
        let ev = run(&cancel());
        assert_eq!(count(&ev, |e| matches!(e, Event::Cancelled { .. })), 1_000);
        assert_eq!(count(&ev, rejected), 0);
    }

    #[test]
    fn amend_is_half_kept_half_lost_and_never_trades() {
        let ev = run(&amend());
        let kept = count(&ev, |e| {
            matches!(
                e,
                Event::Amended {
                    priority: Priority::Kept,
                    ..
                }
            )
        });
        let lost = count(&ev, |e| {
            matches!(
                e,
                Event::Amended {
                    priority: Priority::Lost,
                    ..
                }
            )
        });
        assert_eq!((kept, lost), (500, 500));
        assert_eq!(count(&ev, |e| matches!(e, Event::Trade { .. })), 0);
        assert_eq!(count(&ev, rejected), 0);
    }

    #[test]
    fn cross_fills_exactly_one_maker_per_order() {
        let ev = run(&cross());
        assert_eq!(count(&ev, |e| matches!(e, Event::Trade { .. })), 1_000);
        // Every maker and every taker completes.
        assert_eq!(count(&ev, |e| matches!(e, Event::Filled { .. })), 2_000);
    }

    #[test]
    fn partial_fill_never_completes_a_maker() {
        let ev = run(&partial_fill());
        assert_eq!(count(&ev, |e| matches!(e, Event::Trade { .. })), 1_000);
        // Only the takers complete.
        assert_eq!(count(&ev, |e| matches!(e, Event::Filled { .. })), 1_000);
    }

    #[test]
    fn sweep_takes_fifty_fills_per_order() {
        let ev = run(&sweep());
        assert_eq!(count(&ev, |e| matches!(e, Event::Trade { .. })), 100 * 50);
        assert_eq!(count(&ev, |e| matches!(e, Event::Cancelled { .. })), 0);
    }

    #[test]
    fn mixed_exercises_every_path() {
        let ev = run(&mixed());
        for kind in [
            "accepted",
            "rejected",
            "amended",
            "trade",
            "filled",
            "rested",
            "cancelled",
        ] {
            let n = count(&ev, |e| e.to_string().starts_with(kind));
            assert!(n > 50, "{kind}: {n}");
        }
    }

    #[test]
    fn workloads_are_deterministic() {
        for (a, b) in all().iter().zip(all().iter()) {
            assert_eq!(a.start, b.start, "{}", a.name);
            assert_eq!(a.commands, b.commands, "{}", a.name);
        }
    }
}
