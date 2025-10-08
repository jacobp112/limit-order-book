//! Determinism: identical inputs give identical events and identical state.

mod common;

use std::fs;
use std::path::Path;

use lob::snapshot::fnv1a;
use lob::{Command, Event, MatchingEngine, Snapshot, journal};

struct Outcome {
    events: Vec<Event>,
    snapshot: Vec<u8>,
}

fn replay_from(initial: &Snapshot, commands: &[Command]) -> Outcome {
    let mut engine = MatchingEngine::restore(initial).expect("valid initial state");
    let events = engine.replay(commands.iter().copied());
    Outcome {
        events,
        snapshot: engine.snapshot().encode(),
    }
}

fn empty() -> Snapshot {
    MatchingEngine::new().snapshot()
}

fn event_digest(events: &[Event]) -> u64 {
    let text: String = events.iter().map(|e| format!("{e}\n")).collect();
    fnv1a(text.as_bytes())
}

fn scenario_journals() -> Vec<(String, Vec<Command>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    let mut paths: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "scn"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap();
            let journal: String = text
                .lines()
                .filter_map(|l| l.trim().strip_prefix('>'))
                .map(|c| format!("{c}\n"))
                .collect();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, journal::parse(&journal).unwrap())
        })
        .collect()
}

#[test]
fn replaying_twice_gives_identical_events_and_state() {
    let mut journals = scenario_journals();
    journals.push(("mixed-1".into(), common::mixed_stream(1, 20_000)));
    journals.push(("mixed-2".into(), common::mixed_stream(2, 20_000)));
    for (name, cmds) in &journals {
        let a = replay_from(&empty(), cmds);
        let b = replay_from(&empty(), cmds);
        assert_eq!(a.events, b.events, "{name}: events differ");
        assert_eq!(a.snapshot, b.snapshot, "{name}: final state differs");
    }
}

#[test]
fn generated_stream_exercises_every_event_kind() {
    let out = replay_from(&empty(), &common::mixed_stream(1, 20_000));
    let kinds = [
        "accepted",
        "rejected",
        "amended",
        "trade",
        "filled",
        "rested",
        "cancelled",
    ];
    for kind in kinds {
        let n = out
            .events
            .iter()
            .filter(|e| e.to_string().starts_with(kind))
            .count();
        assert!(n > 100, "only {n} `{kind}` events; stream is too narrow");
    }
    let resting = Snapshot::decode(&out.snapshot).unwrap();
    assert!(!resting.bids.is_empty() && !resting.asks.is_empty());
}

/// replay(S, E1 ++ E2) == replay(restore(snapshot(replay(S, E1))), E2)
#[test]
fn snapshot_restore_composes_with_replay() {
    let cmds = common::mixed_stream(3, 5_000);
    let whole = replay_from(&empty(), &cmds);
    for split in [0, 1, 17, 1_000, 2_500, 4_999, 5_000] {
        let (e1, e2) = cmds.split_at(split);
        let first = replay_from(&empty(), e1);
        let mid = Snapshot::decode(&first.snapshot).expect("engine snapshots are valid");
        let second = replay_from(&mid, e2);
        assert_eq!(
            [first.events, second.events].concat(),
            whole.events,
            "split {split}: events differ"
        );
        assert_eq!(
            second.snapshot, whole.snapshot,
            "split {split}: state differs"
        );
    }
}

#[test]
fn every_intermediate_snapshot_is_valid_and_round_trips() {
    let mut engine = MatchingEngine::new();
    let mut out = Vec::new();
    for cmd in common::mixed_stream(4, 3_000) {
        engine.apply(cmd, &mut out);
        let snap = engine.snapshot();
        let bytes = snap.encode();
        assert_eq!(Snapshot::decode(&bytes).as_ref(), Ok(&snap));
        let restored = MatchingEngine::restore(&snap).unwrap();
        assert_eq!(restored.snapshot().encode(), bytes);
    }
}

#[test]
fn rejected_commands_leave_state_byte_identical() {
    let mut engine = MatchingEngine::new();
    let mut rejected = 0;
    for cmd in common::mixed_stream(5, 5_000) {
        let before = engine.snapshot().encode();
        let mut out = Vec::new();
        engine.apply(cmd, &mut out);
        if let [Event::Rejected { .. }] = out.as_slice() {
            rejected += 1;
            assert_eq!(engine.snapshot().encode(), before, "`{cmd}` changed state");
        } else {
            assert!(
                !out.iter().any(|e| matches!(e, Event::Rejected { .. })),
                "`{cmd}`: rejection mixed with other events: {out:?}"
            );
        }
    }
    assert!(rejected > 200, "only {rejected} rejections exercised");
}

/// Pinned outputs. The same values must be produced on every platform in CI;
/// a change here means matching behaviour or the encodings changed.
#[test]
fn golden_digests() {
    let out = replay_from(&empty(), &common::mixed_stream(1, 20_000));
    let snap = Snapshot::decode(&out.snapshot).unwrap();
    println!(
        "empty={:#018x} events={} event_digest={:#018x} state_digest={:#018x}",
        empty().digest(),
        out.events.len(),
        event_digest(&out.events),
        snap.digest()
    );
    assert_eq!(empty().digest(), GOLDEN_EMPTY);
    assert_eq!(out.events.len(), GOLDEN_MIXED_1.0);
    assert_eq!(event_digest(&out.events), GOLDEN_MIXED_1.1);
    assert_eq!(snap.digest(), GOLDEN_MIXED_1.2);
}

const GOLDEN_EMPTY: u64 = 0x68b9_2c18_fb3f_b5ed;
const GOLDEN_MIXED_1: (usize, u64, u64) = (45_666, 0x2667_77a5_0118_baa5, 0x8bdf_e259_7cb7_cc13);
