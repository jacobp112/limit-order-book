//! Property tests over generated command streams.
//!
//! For every generated stream, after every command, the engine's events must
//! equal the naive reference model's events and invariants I1–I10 must hold
//! (via `Auditor`). At the end the two snapshots must be identical.
//!
//! Failures are shrunk by proptest and reported as a journal that can be
//! replayed with `lob replay`. Failing seeds are persisted under
//! `tests/properties.proptest-regressions`. Run more cases with, e.g.,
//! `PROPTEST_CASES=5000 cargo test --release --test properties`.

mod reference;

use std::fmt::Write as _;

use lob::invariants::Auditor;
use lob::{
    Command, Event, MAX_PRICE, MAX_QTY, MatchingEngine, OrderId, OrderType, Price, Qty, Side,
    Snapshot,
};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use reference::Model;

/// An order reference resolved against ids issued so far, so that shrinking
/// a stream keeps cancels and amends pointed at live orders.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// The k-th most recently issued id (0 = latest).
    Recent(u64),
    /// A literal id, usually one never issued.
    Raw(u64),
}

#[derive(Clone, Copy, Debug)]
enum Spec {
    Submit(Side, OrderType, u64),
    Cancel(Target),
    Amend(Target, Option<i64>, u64),
}

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Buy), Just(Side::Sell)]
}

/// Mostly a narrow band so orders cross and queue; sometimes boundaries.
fn price() -> impl Strategy<Value = i64> {
    prop_oneof![
        9 => 95i64..=105,
        1 => prop_oneof![
            Just(0),
            Just(-1),
            Just(1),
            Just(MAX_PRICE),
            Just(MAX_PRICE + 1),
            Just(i64::MIN),
            Just(i64::MAX),
        ],
    ]
}

/// Mostly small, sometimes large enough to sweep, sometimes invalid.
fn qty() -> impl Strategy<Value = u64> {
    prop_oneof![
        8 => 1u64..=20,
        1 => 21u64..=300,
        1 => prop_oneof![Just(0), Just(MAX_QTY), Just(MAX_QTY + 1), Just(u64::MAX)],
    ]
}

fn target() -> impl Strategy<Value = Target> {
    prop_oneof![
        9 => (0u64..16).prop_map(Target::Recent),
        1 => prop_oneof![Just(0), Just(u64::MAX), any::<u64>()].prop_map(Target::Raw),
    ]
}

fn spec() -> impl Strategy<Value = Spec> {
    let kind = prop_oneof![
        6 => price().prop_map(|p| OrderType::Limit { price: Price(p) }),
        2 => price().prop_map(|p| OrderType::Ioc { price: Price(p) }),
        1 => Just(OrderType::Market),
    ];
    prop_oneof![
        6 => (side(), kind, qty()).prop_map(|(s, k, q)| Spec::Submit(s, k, q)),
        2 => target().prop_map(Spec::Cancel),
        2 => (target(), proptest::option::of(price()), qty())
            .prop_map(|(t, p, q)| Spec::Amend(t, p, q)),
    ]
}

fn stream() -> impl Strategy<Value = Vec<Spec>> {
    proptest::collection::vec(spec(), 1..300)
}

fn resolve(t: Target, issued: u64) -> OrderId {
    match t {
        Target::Recent(k) => OrderId(issued.saturating_sub(k)),
        Target::Raw(id) => OrderId(id),
    }
}

/// Turns specs into concrete commands by running them through the engine,
/// checking model agreement and invariants after each one.
fn check_stream(specs: &[Spec]) -> Result<Vec<Command>, TestCaseError> {
    let mut engine = MatchingEngine::new();
    let mut model = Model::new();
    let mut auditor = Auditor::new();
    let mut issued = 0u64;
    let mut journal = Vec::with_capacity(specs.len());
    let mut out = Vec::new();

    for spec in specs {
        let cmd = match *spec {
            Spec::Submit(side, kind, q) => Command::Submit {
                side,
                kind,
                qty: Qty(q),
            },
            Spec::Cancel(t) => Command::Cancel {
                id: resolve(t, issued),
            },
            Spec::Amend(t, p, q) => Command::Amend {
                id: resolve(t, issued),
                price: p.map(Price),
                qty: Qty(q),
            },
        };
        journal.push(cmd);

        out.clear();
        engine.apply(cmd, &mut out);
        let expected = model.apply(cmd);
        if out != expected {
            return Err(fail(
                &journal,
                "engine and reference model disagree",
                &out,
                &expected,
            ));
        }
        if let Err(v) = auditor.observe(&cmd, &out, engine.book()) {
            return Err(fail(&journal, &v.to_string(), &out, &[]));
        }
        let accepted = out
            .iter()
            .filter(|e| matches!(e, Event::Accepted { .. }))
            .count();
        issued = issued.saturating_add(accepted as u64);
    }

    if engine.snapshot() != model.snapshot() {
        return Err(fail(&journal, "final snapshots differ", &[], &[]));
    }
    auditor
        .reconcile_all(engine.book())
        .map_err(|v| fail(&journal, &v.to_string(), &[], &[]))?;
    Ok(journal)
}

fn fail(journal: &[Command], what: &str, got: &[Event], want: &[Event]) -> TestCaseError {
    let mut msg = format!("{what}\n");
    if !got.is_empty() || !want.is_empty() {
        let lines = |evs: &[Event]| evs.iter().map(|e| format!("\n    {e}")).collect::<String>();
        write!(msg, "  engine:{}\n  model:{}\n", lines(got), lines(want)).unwrap();
    }
    msg.push_str("journal to reproduce:\n");
    for cmd in journal {
        writeln!(msg, "{cmd}").unwrap();
    }
    TestCaseError::fail(msg)
}

fn replay(initial: &Snapshot, cmds: &[Command]) -> (Vec<Event>, Vec<u8>) {
    let mut engine = MatchingEngine::restore(initial).unwrap();
    let events = engine.replay(cmds.iter().copied());
    (events, engine.snapshot().encode())
}

proptest! {
    #[test]
    fn engine_matches_reference_and_keeps_invariants(specs in stream()) {
        check_stream(&specs)?;
    }

    #[test]
    fn replay_is_deterministic_and_composes(
        specs in stream(),
        split in any::<proptest::sample::Index>(),
    ) {
        let cmds = check_stream(&specs)?;
        let empty = MatchingEngine::new().snapshot();
        let whole = replay(&empty, &cmds);
        prop_assert_eq!(&whole, &replay(&empty, &cmds));

        let (e1, e2) = cmds.split_at(split.index(cmds.len().saturating_add(1)));
        let (ev1, bytes1) = replay(&empty, e1);
        let mid = Snapshot::decode(&bytes1).unwrap();
        let (ev2, bytes2) = replay(&mid, e2);
        prop_assert_eq!([ev1, ev2].concat(), whole.0);
        prop_assert_eq!(bytes2, whole.1);
    }
}

/// The oracle itself must be checked against hand-written expectations, or
/// agreement with it would prove little.
#[test]
fn reference_model_agrees_with_scenarios() {
    let journal = "submit sell limit 101 5\nsubmit sell limit 101 3\nsubmit sell limit 102 5\n\
                   submit buy limit 102 10\namend 3 101 5\nsubmit buy market 9\n";
    let mut model = Model::new();
    let text: Vec<String> = lob::journal::parse(journal)
        .unwrap()
        .into_iter()
        .flat_map(|c| model.apply(c))
        .map(|e| e.to_string())
        .collect();
    assert_eq!(
        text,
        [
            "accepted id=1 sell limit 101 qty=5 seq=1",
            "rested id=1 sell px=101 qty=5 seq=1",
            "accepted id=2 sell limit 101 qty=3 seq=2",
            "rested id=2 sell px=101 qty=3 seq=2",
            "accepted id=3 sell limit 102 qty=5 seq=3",
            "rested id=3 sell px=102 qty=5 seq=3",
            "accepted id=4 buy limit 102 qty=10 seq=4",
            "trade maker=1 taker=4 buy px=101 qty=5",
            "filled id=1",
            "trade maker=2 taker=4 buy px=101 qty=3",
            "filled id=2",
            "trade maker=3 taker=4 buy px=102 qty=2",
            "filled id=4",
            "amended id=3 px=101 qty=3->5 seq=5 lost",
            "rested id=3 sell px=101 qty=5 seq=5",
            "accepted id=5 buy market qty=9 seq=6",
            "trade maker=3 taker=5 buy px=101 qty=5",
            "filled id=3",
            "cancelled id=5 qty=4 no-liquidity",
        ]
    );
}
