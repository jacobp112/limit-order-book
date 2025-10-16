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
| Benchmark | `crates/lob-bench` (Criterion and a latency harness) | performance characteristics; see [benchmarks.md](benchmarks.md) |

## Scenario tests

Scenario files (`crates/lob/tests/scenarios/*.scn`) interleave journal
commands with the exact events expected and, optionally, the whole book as a
price ladder afterwards; the format is described in `tests/scenarios.rs`. A
mismatch reports the line, both event lists and the journal prefix that
reproduces it. Edge cases found during development become scenario tests.

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

Quality gates before every commit (CI runs the same on Linux and Windows):

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release --manifest-path fuzz/Cargo.toml   # fuzz harness on seeds
node explainer/check.mjs                              # wasm engine vs CLI, explainer model vs engine
```

## Benchmarks

Workloads live in `crates/lob-bench` so the library carries no benchmark
code. Each is a starting snapshot plus a fixed command batch built from a
fixed seed; the benchmark restores the snapshot outside the timed region and
times the batch. Unit tests check that each workload does what its name says
(for example, `cross` produces exactly one full fill per order and `amend`
exactly 500 kept and 500 lost priorities), so a generator mistake cannot
silently change what is measured.

| Workload | Batch | Starting book |
|---|---|---|
| `rest_insert` | 1,000 non-crossing limits joining existing levels | 100 levels × 10 orders per side |
| `cross` | 1,000 limits, each fully filling one maker | 10,000 one-lot asks over 100 levels |
| `cancel` | 1,000 cancels of distinct random orders | 20,000 resting, 100 per level |
| `amend` | 1,000 amends: 500 size-down in place, 500 re-price | 20,000 resting |
| `partial_fill` | 1,000 small takers partially filling one large maker | 10 large asks at one level |
| `sweep` | 100 market orders, each taking 10 levels (50 fills) | 1,000 levels × 5 orders |
| `mixed` | 10,000 commands of mixed flow | state after 20,000 commands of the same flow |

```
cargo bench -p lob-bench --bench engine
```

Criterion uses 2 s warm-up, 10 s measurement and 50 samples per workload,
and reports time per batch with a confidence interval and throughput per
command.

Criterion times batches, so a separate harness (`--bin latency`) records
the latency distribution from timed chunks of consecutive commands; the
timer resolution and overhead are measured and recorded with each run.
Method, conditions, results and caveats are in [benchmarks.md](benchmarks.md).
