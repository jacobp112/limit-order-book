# Testing and measurement strategy

| Layer | Where | Purpose |
|---|---|---|
| Unit | `#[cfg(test)]` in each module | types, level linking, book operations, validation |
| Scenario | `crates/lob/tests/` | realistic sequences with exact expected events |
| Replay | `crates/lob/tests/` | identical events and snapshot bytes across runs; snapshot/restore composition |
| Property | `crates/lob/tests/` (proptest) | invariants I1–I10 after every command; differential check against a naive reference book |
| Fuzz | `fuzz/` (cargo-fuzz) | hostile and malformed streams; run separately, not in `cargo test` |
| Benchmark | `crates/lob/benches/` (Criterion) and a latency harness | performance characteristics |

Property testing uses `proptest` rather than `quickcheck`: its strategies
compose well for command streams, shrinking operates on the generated
structure, and failing cases are persisted in `proptest-regressions/`.

Fuzzing uses `cargo-fuzz` (libFuzzer) with `arbitrary`. libFuzzer's Windows
support is experimental, so fuzzing is run in a Linux container.

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
