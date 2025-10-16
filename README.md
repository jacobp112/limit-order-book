# limit-order-book

A single-instrument, in-memory limit order book and matching engine in Rust.
Orders match by price, then by arrival time within a price. Given the same
starting state and the same commands, the engine produces the same events
and the same final state, byte for byte, on every run and platform.

```
$ cargo run -q -p lob-cli -- replay examples/journals/session.journal
> submit sell limit 101 5
  accepted id=1 sell limit 101 qty=5 seq=1
  rested id=1 sell px=101 qty=5 seq=1
...
> submit buy limit 101 7
  accepted id=6 buy limit 101 qty=7 seq=6
  trade maker=1 taker=6 buy px=101 qty=5
  filled id=1
  trade maker=3 taker=6 buy px=101 qty=2
  filled id=6
...
commands 12  events 31  trades 6  resting 0
event digest 0x3992e01f91960e21
state digest 0x99d887fd8c46ef4e
```

## What it does

- Limit, market and immediate-or-cancel (IOC) orders; cancel; amend.
- Partial fills and sweeps across any number of price levels.
- Every command produces an explicit event stream: accepted, rejected,
  trade, filled, rested, cancelled, amended.
- Invalid commands are rejected with a specific reason and change nothing.
- A command journal (plain text) and a canonical binary snapshot, so any run
  can be replayed or resumed exactly.

## What it deliberately does not do

No networking, FIX, sessions or authentication; no persistence beyond
journals and snapshots; one instrument; no fees, risk checks, self-trade
prevention, stop/FOK/iceberg orders or wall-clock timestamps. Details and the
reasons are in [requirements](docs/requirements.md#scope).

## How matching works

An incoming order trades against the opposite side while prices cross:
best price first, and within a price the earliest resting order first. Each
trade executes at the resting order's price. What is left of a limit order
rests at the back of its price's queue; what is left of an IOC or market
order is cancelled. Amending a resting order to a smaller size at the same
price keeps its place; a larger size or a new price moves it to the back, and
a new price that crosses trades immediately. Order ids and sequence numbers
are assigned by the engine.

## Data structures

| | Structure | Why |
|---|---|---|
| Each side | `BTreeMap<Price, Level>` | best price and level-by-level sweeps in order, deterministic iteration |
| Each level | intrusive doubly linked FIFO over an arena of `u32` handles | O(1) append, head removal and mid-queue cancel; no per-order allocation after warm-up |
| Id lookup | `HashMap<OrderId, Handle>` with a fixed hasher, never iterated | O(1) average cancel/amend lookup without affecting output order |

Best bid/ask is O(log L) for L levels; resting insert O(log L); cancel O(1)
average; matching O(k + m·log L) for k fills that empty m levels. Prices
(ticks) and quantities (lots) are integers with explicit bounds, so level
totals cannot overflow. More in [design](docs/design.md).

## Determinism

The engine has no clock, randomness, concurrency or iteration over unordered
collections on any output path. A snapshot holds everything that affects
future output (resting orders in priority order plus the id and sequence
counters) with a fixed little-endian encoding. Tests replay journals twice
and compare events and snapshot bytes; check that
`replay(S, E1 ++ E2) == replay(restore(snapshot(replay(S, E1))), E2)`; and
pin digests of a 20,000-command generated stream that CI must reproduce on
both Linux and Windows.

## Conservation invariants

For every order, accepted = filled + open + cancelled, with the open quantity
taken from the book rather than from the engine's own accounting; buy fills
equal sell fills equal traded quantity; rejections have no effect; levels,
queues and the id index agree; the resting book is never crossed; trades
execute at the resting price, within the incoming order's limit, in
price-time order. The ten invariants are stated in
[requirements](docs/requirements.md#invariants) and checked by
`OrderBook::check_structure` and an event-driven `invariants::Auditor`.

## Testing

| Layer | What it shows |
|---|---|
| Unit, behaviour, scenario tests | each rule, through the public API and through readable scenario files with exact expected events |
| Invariant audit | I1–I10 hold after every command of generated streams; forged events and corrupted books are rejected |
| Property tests (proptest) | on generated streams the engine's events equal those of a deliberately naive reference model, and the audit passes; failures shrink to a replayable journal |
| Replay tests | determinism and snapshot/restore composition, as above |

An injected pricing bug is caught by the audit on the fourth command of a
generated stream, and an injected IOC bug that the audit cannot see is
caught by the reference model and shrunk to one command. See
[testing](docs/testing.md).

## Fuzzing

Three cargo-fuzz targets (structured command streams, journal text, snapshot
bytes) run every accepted input under the invariant audit. The snapshot
target found a real bug: a snapshot could set a counter one step from
overflow and make the next command panic. It is fixed, and the input is kept
as a regression test. Campaign numbers and how to run them are in
[fuzz/README.md](fuzz/README.md).

## Benchmarks

Seven fixed-seed workloads (resting insert, single-fill cross, cancel,
amend, partial fills, 50-fill sweep, mixed flow), measured with Criterion and
with a latency-percentile harness. On one laptop with the CPU near its 2.4 GHz
base clock, costs ranged from about 30 ns for a partial fill to about 450 ns
for a cancel or amend in a 20,000-order book, and about 110 ns per fill in a
sweep. Absolute numbers on that machine moved by 2–3× with CPU boost, which
is documented along with the method, raw data and charts in
[benchmarks](docs/benchmarks.md).

## Visual explainer

[`explainer/`](explainer/README.md) is a small page that runs the engine
compiled to WebAssembly. Load an example journal or type commands, then step
through the engine's events one at a time and watch which resting order each
trade hits. It has no matching logic of its own; a CI check verifies its
displayed book against the engine after every command of every seed journal.

## Repository

```
crates/lob        engine library (no dependencies, no unsafe)
crates/lob-cli    `lob replay`
crates/lob-bench  benchmark workloads, Criterion suite, latency harness
crates/lob-wasm   WebAssembly exports for the explainer
fuzz/             cargo-fuzz targets and seed corpus
explainer/        step-through page
examples/         sample journals
docs/             requirements, design, testing, benchmarks, backlog
tools/            benchmark aggregation and charts
```

```
cargo test --workspace                                   # all tests
cargo run -p lob-cli -- replay <journal> [--quiet] [--save f] [--from f]
cargo bench -p lob-bench --bench engine                  # Criterion
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
