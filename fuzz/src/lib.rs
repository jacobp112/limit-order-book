//! Fuzz harnesses for the lob engine.
//!
//! Each harness takes raw bytes and panics if anything is wrong. The
//! `fuzz_targets/` binaries wrap them for libFuzzer; `tests/seeds.rs` runs
//! them over the seed corpus on stable so the harness logic itself is tested.
//!
//! What counts as wrong:
//! - any panic inside the engine, parser or decoder;
//! - any invariant violation reported by `lob::invariants::Auditor`;
//! - replay of the same input producing different results;
//! - a snapshot that decodes but does not re-encode to the same bytes, or
//!   restores to a different state.

use arbitrary::{Arbitrary, Result, Unstructured};
use lob::invariants::Auditor;
use lob::journal;
use lob::{Command, Event, MatchingEngine, OrderId, OrderType, Price, Qty, Side, Snapshot};

/// How a fuzzed command refers to an order.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum Target {
    /// k-th most recently issued id, so fuzzed streams keep hitting live,
    /// filled and cancelled orders.
    Recent(u8),
    /// Any id at all: never issued, zero, `u64::MAX`.
    Raw(u64),
}

/// A price: usually near a common mid so orders cross, sometimes anything.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum FuzzPrice {
    Near(i8),
    Raw(i64),
}

/// A quantity: usually small so partial fills happen, sometimes anything.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum FuzzQty {
    Small(u8),
    Raw(u64),
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum FuzzKind {
    Limit(FuzzPrice),
    Ioc(FuzzPrice),
    Market,
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum FuzzCommand {
    Submit(bool, FuzzKind, FuzzQty),
    Cancel(Target),
    Amend(Target, Option<FuzzPrice>, FuzzQty),
}

impl FuzzPrice {
    fn resolve(self) -> Price {
        match self {
            Self::Near(d) => Price(1_000 + i64::from(d)),
            Self::Raw(p) => Price(p),
        }
    }
}

impl FuzzQty {
    fn resolve(self) -> Qty {
        match self {
            Self::Small(q) => Qty(u64::from(q)),
            Self::Raw(q) => Qty(q),
        }
    }
}

impl FuzzCommand {
    fn resolve(self, issued: u64) -> Command {
        let id = |t: Target| match t {
            Target::Recent(k) => OrderId(issued.saturating_sub(u64::from(k))),
            Target::Raw(id) => OrderId(id),
        };
        match self {
            Self::Submit(buy, kind, qty) => Command::Submit {
                side: if buy { Side::Buy } else { Side::Sell },
                kind: match kind {
                    FuzzKind::Limit(p) => OrderType::Limit { price: p.resolve() },
                    FuzzKind::Ioc(p) => OrderType::Ioc { price: p.resolve() },
                    FuzzKind::Market => OrderType::Market,
                },
                qty: qty.resolve(),
            },
            Self::Cancel(t) => Command::Cancel { id: id(t) },
            Self::Amend(t, p, q) => Command::Amend {
                id: id(t),
                price: p.map(FuzzPrice::resolve),
                qty: q.resolve(),
            },
        }
    }
}

/// Decodes as many commands as the bytes allow.
fn decode_commands(data: &[u8]) -> Result<Vec<FuzzCommand>> {
    let mut u = Unstructured::new(data);
    let mut out = Vec::new();
    while !u.is_empty() {
        out.push(u.arbitrary()?);
    }
    Ok(out)
}

/// Runs commands with a full audit after each one, and returns the events
/// and final snapshot bytes.
fn run_audited(start: &Snapshot, cmds: &[Command]) -> (Vec<Event>, Vec<u8>) {
    let mut engine = MatchingEngine::restore(start).expect("valid start state");
    let mut auditor = Auditor::from_snapshot(start);
    let mut all = Vec::new();
    let mut out = Vec::new();
    for (i, cmd) in cmds.iter().enumerate() {
        out.clear();
        engine.apply(*cmd, &mut out);
        if let Err(v) = auditor.observe(cmd, &out, engine.book()) {
            let journal: String = cmds[..=i].iter().map(|c| format!("{c}\n")).collect();
            panic!("invariant violated: {v}\njournal:\n{journal}");
        }
        all.extend_from_slice(&out);
    }
    auditor
        .reconcile_all(engine.book())
        .unwrap_or_else(|v| panic!("final reconcile failed: {v}"));
    (all, engine.snapshot().encode())
}

/// Structured command streams: every command is audited, and the whole
/// stream must replay identically, including across a mid-stream snapshot.
pub fn commands(data: &[u8]) {
    let Ok(fuzzed) = decode_commands(data) else {
        return;
    };
    let mut issued = 0u64;
    let mut engine = MatchingEngine::new();
    let mut cmds = Vec::with_capacity(fuzzed.len());
    let mut out = Vec::new();
    for f in fuzzed {
        let cmd = f.resolve(issued);
        out.clear();
        engine.apply(cmd, &mut out);
        let accepted = out
            .iter()
            .filter(|e| matches!(e, Event::Accepted { .. }))
            .count();
        issued = issued.saturating_add(accepted as u64);
        cmds.push(cmd);
    }

    let empty = MatchingEngine::new().snapshot();
    let whole = run_audited(&empty, &cmds);
    assert_eq!(
        whole.1,
        engine.snapshot().encode(),
        "audited rerun diverged"
    );

    let split = cmds.len() / 2;
    let (first, second) = cmds.split_at(split);
    let (ev1, mid) = run_audited(&empty, first);
    let mid = Snapshot::decode(&mid).expect("engine snapshot must decode");
    let (ev2, end) = run_audited(&mid, second);
    assert_eq!(
        [ev1, ev2].concat(),
        whole.0,
        "events differ across snapshot split"
    );
    assert_eq!(end, whole.1, "state differs across snapshot split");
}

/// Arbitrary text through the journal parser. Parsing must not panic;
/// anything that parses must print and re-parse to the same commands and
/// run cleanly under audit.
pub fn journal_text(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    match journal::parse(&text) {
        Ok(cmds) => {
            let printed: String = cmds.iter().map(|c| format!("{c}\n")).collect();
            assert_eq!(
                journal::parse(&printed).as_ref(),
                Ok(&cmds),
                "print/parse round trip"
            );
            run_audited(&MatchingEngine::new().snapshot(), &cmds);
        }
        Err(e) => {
            assert!(
                e.line >= 1 && e.line <= text.lines().count(),
                "bad error line {e}"
            );
            let _ = e.to_string();
        }
    }
}

/// Arbitrary bytes through the snapshot decoder. Decoding must not panic;
/// anything accepted must be canonical, restorable, structurally sound and
/// usable as a starting point.
pub fn snapshot_bytes(data: &[u8]) {
    let Ok(snap) = Snapshot::decode(data) else {
        return;
    };
    assert_eq!(snap.encode(), data, "accepted snapshot is not canonical");
    let engine = MatchingEngine::restore(&snap).expect("decoded snapshot must restore");
    engine
        .book()
        .check_structure()
        .unwrap_or_else(|v| panic!("restored book broken: {v}"));
    assert_eq!(engine.snapshot(), snap, "restore changed the state");

    // Trade through whatever was restored from both sides.
    let sweep = |side| Command::Submit {
        side,
        kind: OrderType::Market,
        qty: Qty(lob::MAX_QTY),
    };
    run_audited(&snap, &[sweep(Side::Buy), sweep(Side::Sell)]);
}
