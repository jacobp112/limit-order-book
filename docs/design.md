# Design

## Layout

```
crates/lob        core library, no runtime dependencies
crates/lob-cli    `lob replay`
crates/lob-bench  deterministic workloads, Criterion benches, latency harness
fuzz/             cargo-fuzz targets, outside the workspace
examples/         sample journals
crates/lob-wasm   WebAssembly exports of the engine for the explainer
explainer/        JS wrapper and Node check (page planned)
tools/            benchmark aggregation and charts
docs/bench/       published benchmark data and charts
```

Core modules:

| Module | Responsibility |
|---|---|
| `types` | `Price`, `Qty`, `OrderId`, `Seq`, `Side`, `OrderType` |
| `command` | `Command`, `Event`, `RejectReason`, `CancelReason` |
| `level` | one price level: an intrusive FIFO list over the order arena |
| `book` | both sides, the order arena and the id index |
| `engine` | validation, matching, event emission, id/sequence counters |
| `journal` | line-based text encoding of commands; stable text form of events |
| `snapshot` | canonical state encoding, digest, validation for restore |
| `invariants` | `Auditor`: event-derived ledger reconciled with the book (I1–I3, I9, I10) |

## Data structures

```
OrderBook
 ├─ bids: BTreeMap<Price, Level>     best = last entry
 ├─ asks: BTreeMap<Price, Level>     best = first entry
 ├─ arena: Vec<Node>, free list      Node { id, side, price, open, seq, prev, next }
 └─ index: HashMap<OrderId, Handle>  Handle = u32 slot in arena

Level { head, tail: Option<Handle>, total: Qty, count: u32 }
```

**Why a `BTreeMap` of levels.** Ordered iteration gives best price and
level-by-level sweeps directly, it has no hashing, and its iteration order is
deterministic. A tick-indexed array would give O(1) best-price lookup but needs
a bounded, dense price range; the order book here makes no such assumption.

**Why an arena-backed intrusive list per level.** FIFO needs O(1) append at the
tail and O(1) removal from the head; cancellation needs O(1) removal from the
middle. A `VecDeque` per level makes mid-queue cancellation O(n). Storing nodes
in one `Vec` with `u32` links keeps them contiguous and avoids per-order
allocation after warm-up.

**Why a `HashMap` index.** Cancel and amend look up by id. Iteration order of
the map is never observed, so its hashing does not affect output.

| Operation | Cost (L = levels on a side, k = fills, m = levels emptied) |
|---|---|
| best bid / ask | O(log L) |
| rest a new order | O(log L) to find or create the level, O(1) append |
| cancel | O(1) average lookup and unlink, O(log L) if the level empties |
| amend, quantity down | O(1) average |
| amend, other | cancel + re-insert |
| match | O(k + m·log L) |

## Engine flow

```
apply(cmd):
  validate(cmd) ─ invalid ─▶ Rejected, return
  Submit: assign id + seq, emit Accepted
          match against opposite side while crossing (maker price, FIFO)
          remainder: Limit → rest; Ioc/Market → Cancelled
  Cancel: unlink, emit Cancelled
  Amend:  qty down, same price → adjust in place
          otherwise → unlink, new seq, then as Submit (may trade)
```

Events are appended to a caller-supplied `Vec<Event>` so steady-state matching
does not allocate per command.

## Determinism

The engine is a function of its state and the next command. Order ids and
sequence numbers come from counters inside that state. There is no clock,
randomness, concurrency or iteration over unordered collections on the path
that produces events or snapshots.

A snapshot holds both counters and, for each side in price order and each
level in FIFO order, every resting order with its id, price, open quantity
and sequence number. That is all state that affects future output, so two
engines with equal snapshots behave identically. Its canonical byte encoding
(little-endian, fixed field order) is compared directly in tests; a 64-bit
FNV-1a digest is provided for compact reporting.

`MatchingEngine::restore` validates a snapshot before using it: counters in
`1..=2^63` (so a restored engine is never close to exhausting them), value
ranges, ids and sequence numbers already issued, no duplicates, priority
order and an uncrossed book. Bytes from outside therefore cannot build a book
the engine could not have reached. Tests check:

- replaying the same journal twice gives identical events and snapshot bytes;
- `replay(S, E1 ++ E2) == replay(restore(snapshot(replay(S, E1))), E2)` at
  several split points;
- every intermediate snapshot decodes and restores to the same bytes;
- pinned digests for a 20,000-command generated stream, so CI on Linux and
  Windows must agree.

## WebAssembly boundary

`lob-wasm` exposes the unmodified engine to JavaScript through a few C-ABI
functions and no binding generator. JavaScript writes one journal line into a
buffer owned by the module and calls `lob_apply`; the module parses it with
the same journal parser as `lob replay`, applies it, and writes a JSON
document with the events (each including its stable text form) and the whole
book. The explainer therefore cannot drift from the engine's behaviour: it has
no matching logic of its own. A Node check (`explainer/check.mjs`), run in CI
against a freshly built module and against the committed one, replays the
example session and requires the same state and event digests as the CLI.

The size-optimised build uses the `wasm` profile
(`cargo build -p lob-wasm --profile wasm --target wasm32-unknown-unknown`,
about 73 KB). The only `unsafe` in the workspace is `#[unsafe(no_mangle)]` on
these exports; the core crate forbids `unsafe`.

## Invariant checking

Two independent checks cover the invariants in `requirements.md`:

- `OrderBook::check_structure` walks the book's internals: cached level
  totals and counts, doubly linked list consistency (with a bounded walk so a
  corrupted cycle is reported rather than looping), index/level agreement,
  positive open quantity, FIFO sequence order and an uncrossed book (I4–I8).
- `invariants::Auditor` rebuilds per-order accepted, filled and cancelled
  quantities purely from commands and events, checks trade prices and
  priority as they stream past (I2, I3, I9, I10), and reconciles against the
  open quantity the *book* reports: `A = F + O + C` (I1).

The auditor does not read engine internals, so a bookkeeping error in the
engine cannot also mask itself. Tests show both checks reject deliberately
corrupted books and forged event streams, and a trade-price bug injected
into the matching loop is caught on the fourth command of a generated
stream.
