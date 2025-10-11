# Testing and measurement strategy

| Layer | Where | Purpose |
|---|---|---|
| Unit | `#[cfg(test)]` in each module | types, level linking, book operations, validation, structural corruption detection |
| Behaviour | `tests/matching.rs` | each user story through the public API |
| Scenario | `tests/scenarios.rs` + `tests/scenarios/*.scn` | realistic sequences with exact expected events and book ladders |
| Replay | `tests/replay.rs` | identical events and snapshot bytes across runs; snapshot/restore composition; pinned cross-platform digests |
| Invariants | `tests/invariants.rs` | I1–I10 after every command of generated streams; forged events are rejected |
| Property | `tests/properties.rs` (proptest) | differential check against a naive reference model plus I1–I10, on generated streams |
| Fuzz | `fuzz/` (cargo-fuzz) | hostile and malformed streams; run separately, not in `cargo test` |
| Benchmark | `crates/lob/benches/` (Criterion) and a latency harness | performance characteristics |

## Property tests

Property testing uses `proptest` rather than `quickcheck`: its strategies
compose well for command streams, shrinking operates on the generated
structure, and failing cases are persisted next to the test file
(`*.proptest-regressions`) and re-run first.

Each case is a stream of up to 300 commands. Prices mostly fall in a narrow
band so orders cross and queue; about one value in ten is a boundary or
invalid value (0, negative, `MAX_PRICE`, `MAX_PRICE + 1`, `i64::MIN`, zero or
oversized quantities). Cancels and amends refer to "the k-th most recently
issued id", resolved while the stream runs, so shrinking keeps them pointed
at live orders; some use raw ids that were never issued.

After every command the engine's events must equal those of
`tests/reference/`, a naive model that keeps resting orders in a flat `Vec`
and scans it for every match, and the `Auditor` must accept them. The final
snapshots must be equal. A second property replays each stream twice and
checks snapshot/restore composition at a random split point. A failure is
reported as a shrunk journal, e.g. an injected IOC cancel-reason bug shrank
to the single command `submit buy ioc 95 1`.

The model is itself checked against a hand-written expectation so that
agreement with it means something.

`cargo test` runs 256 cases per property. For a longer run:

```
PROPTEST_CASES=5000 cargo test --release -p lob --test properties
```

## Fuzzing

Fuzzing uses `cargo-fuzz` (libFuzzer) with `arbitrary`, in `fuzz/`, outside
the main workspace. Targets cover structured command streams, the journal
parser and the snapshot decoder; every accepted input is run under the full
invariant audit. libFuzzer's Windows support is experimental, so fuzzing runs
in a Linux container; the harness logic is also exercised on stable by
`cargo test` in `fuzz/`. See `fuzz/README.md` for targets and commands.

Scenario files (`crates/lob/tests/scenarios/*.scn`) interleave journal
commands with the exact events expected and, optionally, the whole book as a
price ladder afterwards; the format is described in `tests/scenarios.rs`. A
mismatch reports the line, both event lists and the journal prefix that
reproduces it. Edge cases found during development become scenario tests.

Quality gates before every commit:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Benchmarks

Criterion measures resting insertion, crossing orders, cancellation,
amendment, partial-fill-heavy matching, multi-level sweeps and mixed synthetic
flow, with warm-up and repeated samples. Workloads are generated from a fixed
seed.

Criterion times batches, so a separate harness records per-operation latency
for median, p95 and p99. On Windows the timer resolution is about 100 ns, so
the harness times small fixed batches and reports per-operation means of those
batches; this is stated alongside every result. Hardware, build profile,
workload, sample size and commit are recorded with each published run.
