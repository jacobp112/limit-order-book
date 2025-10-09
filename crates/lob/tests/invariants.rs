//! Invariants I1–I10 checked after every command of long generated streams,
//! and evidence that the auditor rejects event streams that break them.

mod common;

use lob::invariants::Auditor;
use lob::{
    CancelReason, Command, Event, MatchingEngine, OrderId, OrderType, Price, Priority, Qty,
    RejectReason, Seq, Side,
};

fn audit(engine: &mut MatchingEngine, auditor: &mut Auditor, commands: &[Command]) {
    let mut out = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        out.clear();
        engine.apply(*cmd, &mut out);
        if let Err(v) = auditor.observe(cmd, &out, engine.book()) {
            let journal: String = commands[..=i].iter().map(|c| format!("{c}\n")).collect();
            panic!("after command {i} `{cmd}`: {v}\nevents: {out:?}\njournal:\n{journal}");
        }
    }
    auditor.reconcile_all(engine.book()).unwrap();
}

/// Every command is fully checked, and the structural check is O(resting
/// orders), so stream length is kept moderate to keep `cargo test` quick.
#[test]
fn invariants_hold_across_generated_streams() {
    for seed in 1..=4 {
        let mut engine = MatchingEngine::new();
        let mut auditor = Auditor::new();
        audit(
            &mut engine,
            &mut auditor,
            &common::mixed_stream(seed, 5_000),
        );
        assert!(auditor.traded() > 0, "seed {seed} produced no trades");
    }
}

#[test]
fn invariants_hold_after_restoring_a_snapshot() {
    let cmds = common::mixed_stream(9, 6_000);
    let (first, second) = cmds.split_at(3_000);
    let mut engine = MatchingEngine::new();
    let mut auditor = Auditor::new();
    audit(&mut engine, &mut auditor, first);

    let snap = engine.snapshot();
    let mut restored = MatchingEngine::restore(&snap).unwrap();
    let mut fresh_auditor = Auditor::from_snapshot(&snap);
    audit(&mut restored, &mut fresh_auditor, second);
}

// The auditor must reject streams that break the rules. Each case replays a
// valid prefix, then feeds a forged set of events for one command.

struct Forge {
    engine: MatchingEngine,
    auditor: Auditor,
}

impl Forge {
    fn new(setup: &[Command]) -> Self {
        let mut engine = MatchingEngine::new();
        let mut auditor = Auditor::new();
        audit(&mut engine, &mut auditor, setup);
        Self { engine, auditor }
    }

    fn violation(&mut self, cmd: Command, events: &[Event]) -> &'static str {
        self.auditor
            .observe(&cmd, events, self.engine.book())
            .expect_err("forged events should be rejected")
            .invariant
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

fn accepted(id: u64, side: Side, price: i64, qty: u64) -> Event {
    Event::Accepted {
        id: OrderId(id),
        side,
        kind: OrderType::Limit {
            price: Price(price),
        },
        qty: Qty(qty),
        seq: Seq(id),
    }
}

fn trade(maker: u64, taker: u64, side: Side, price: i64, qty: u64) -> Event {
    Event::Trade {
        maker: OrderId(maker),
        taker: OrderId(taker),
        taker_side: side,
        price: Price(price),
        qty: Qty(qty),
    }
}

fn two_asks() -> Forge {
    Forge::new(&[limit(Side::Sell, 101, 5), limit(Side::Sell, 102, 5)])
}

#[test]
fn forged_rejection_with_side_effects_is_i3() {
    let mut f = two_asks();
    let cmd = limit(Side::Buy, 0, 1);
    let events = [
        Event::Rejected {
            id: None,
            reason: RejectReason::PriceOutOfRange,
        },
        accepted(3, Side::Buy, 0, 1),
    ];
    assert_eq!(f.violation(cmd, &events), "I3");
}

#[test]
fn trade_away_from_maker_price_is_i9() {
    let mut f = two_asks();
    let cmd = limit(Side::Buy, 105, 1);
    let events = [
        accepted(3, Side::Buy, 105, 1),
        trade(1, 3, Side::Buy, 103, 1),
    ];
    assert_eq!(f.violation(cmd, &events), "I9");
}

#[test]
fn trade_through_taker_limit_is_i9() {
    let mut f = two_asks();
    let cmd = limit(Side::Buy, 101, 1);
    let events = [
        accepted(3, Side::Buy, 101, 1),
        trade(2, 3, Side::Buy, 102, 1),
    ];
    assert_eq!(f.violation(cmd, &events), "I9");
}

#[test]
fn filling_a_worse_price_first_is_i10() {
    let mut f = two_asks();
    let cmd = limit(Side::Buy, 102, 2);
    let events = [
        accepted(3, Side::Buy, 102, 2),
        trade(2, 3, Side::Buy, 102, 1),
        trade(1, 3, Side::Buy, 101, 1),
    ];
    assert_eq!(f.violation(cmd, &events), "I10");
}

#[test]
fn filled_with_quantity_left_is_i1() {
    let mut f = two_asks();
    let cmd = limit(Side::Buy, 101, 3);
    let events = [
        accepted(3, Side::Buy, 101, 3),
        trade(1, 3, Side::Buy, 101, 2),
        Event::Filled { id: OrderId(3) },
    ];
    assert_eq!(f.violation(cmd, &events), "I1");
}

#[test]
fn quantity_that_vanishes_is_i1() {
    // Cancel reports less than was open: 2 lots would disappear.
    let mut f = two_asks();
    let cmd = Command::Cancel { id: OrderId(1) };
    let events = [Event::Cancelled {
        id: OrderId(1),
        qty: Qty(3),
        reason: CancelReason::User,
    }];
    assert_eq!(f.violation(cmd, &events), "I1");
}

#[test]
fn amend_misreporting_old_quantity_is_i1() {
    let mut f = two_asks();
    let cmd = Command::Amend {
        id: OrderId(1),
        price: None,
        qty: Qty(2),
    };
    let events = [Event::Amended {
        id: OrderId(1),
        price: Price(101),
        old_qty: Qty(4),
        qty: Qty(2),
        seq: Seq(1),
        priority: Priority::Kept,
    }];
    assert_eq!(f.violation(cmd, &events), "I1");
}

#[test]
fn events_that_disagree_with_the_book_are_caught() {
    // The events claim an order rested, but the book was never changed.
    let mut f = two_asks();
    let cmd = limit(Side::Buy, 90, 1);
    let events = [
        accepted(3, Side::Buy, 90, 1),
        Event::Rested {
            id: OrderId(3),
            side: Side::Buy,
            price: Price(90),
            qty: Qty(1),
            seq: Seq(3),
        },
    ];
    assert_eq!(f.violation(cmd, &events), "I1");
}
