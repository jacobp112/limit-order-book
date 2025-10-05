# Backlog

Status: `todo`, `doing`, `done`.

## US-001 — Submit limit order · done
As a market participant, I want to submit a limit order so that it rests on the
book until it can trade at its specified price or better.
- A non-crossing limit order is acknowledged with a new order id and rests.
- It appears in depth at its price, with the level total increased by its quantity.
- Best bid/ask reflect it when it improves the top of book.

## US-002 — Trade at the best available price · done
As a participant submitting a crossing order, I want it to trade against the
best-priced opposite orders first, at their prices.
- A buy crossing several ask levels fills against the lowest ask first.
- Each trade reports maker id, taker id, price and quantity; the price is the maker's.
- A buy never trades above its limit; a sell never trades below it.

## US-003 — Partial fills across levels · done
As a participant, I want a large order to fill as much as possible and keep the rest.
- An order larger than the best level continues into the next level.
- A partially filled maker stays at the head of its queue with reduced quantity.
- An unfilled limit remainder rests at its limit price.

## US-004 — Time priority within a level · done
As a participant, I want orders at the same price filled in arrival order.
- Given A then B at the same price, an incoming order fills A before B.

## US-005 — Market order · done
As a participant, I want a market order that takes available liquidity at any price.
- It fills across levels until complete or the opposite side is empty.
- Any unfilled remainder is cancelled with reason `NoLiquidity`; nothing rests.

## US-006 — Immediate-or-cancel order · done
As a participant, I want an IOC order that trades up to its limit immediately and never rests.
- It fills only at prices within its limit.
- Any remainder is cancelled with reason `IocRemainder`.

## US-007 — Cancel order · done
As a participant, I want to cancel my resting order.
- The order leaves the book; the event reports the open quantity cancelled.
- Cancelling an unknown, filled or already cancelled order is rejected with `UnknownOrder`.

## US-008 — Amend order · done
As a participant, I want to change the price or open quantity of my resting order.
- Reducing quantity at the same price keeps queue position.
- Increasing quantity, or changing price, moves the order to the back of the queue.
- A new price that crosses trades immediately; any remainder rests.
- Amending to zero, or to the current values, is rejected.

## US-009 — Safe rejection of invalid commands · doing
As an operator, I want invalid commands rejected without side effects.
- Each invalid command yields one `Rejected` event with a specific reason.
- The book snapshot is byte-identical before and after.

## US-010 — Inspect the book · done
As a participant, I want to see top of book and depth.
- Best bid and ask, and per-level totals in price order, are available.
- Per-order detail (id, quantity) is available in queue order for each level.

## US-011 — Deterministic replay · todo
As a developer, I want to replay a recorded command journal and get identical results.
- Replaying a journal twice yields identical events and identical snapshot bytes.
- A CLI prints the events and the state digest for a journal file.

## US-012 — Snapshot and restore · todo
As a developer, I want to restore a snapshot and continue from it.
- Replaying E1 then E2 equals restoring the snapshot after E1, then replaying E2.

## US-013 — Conservation under randomised flow · todo
As the maintainer, I want long random command streams checked against the invariants.
- Streams mix limit, market, IOC, cancel and amend commands.
- Invariants I1–I10 hold after every command.
- Results match a simple reference implementation exactly.
- A failure prints a minimal reproducing journal.

## US-014 — Hostile input fuzzing · todo
As the maintainer, I want fuzzed command streams to never crash or corrupt state.
- Fuzz targets cover structured commands and the journal parser.
- Invariants are checked after every command.

## US-015 — Reproducible benchmarks · todo
As a reviewer, I want to reproduce the performance figures.
- One documented command runs the suite and writes machine-readable results.
- Hardware, build profile, workload and sample sizes are recorded.

## US-016 — Latency charts · todo
As a reviewer, I want latency distributions shown as charts.
- A script renders median, p95 and p99 from the benchmark output.

## US-017 — Visual explainer · todo
As a visitor, I want to submit orders and watch how they match.
- Bids and asks are shown by level with per-order queues.
- Submitted orders animate through the fills they produce; trades are listed.
- Behaviour comes from the real engine compiled to WebAssembly.

## US-018 — Reviewer documentation · todo
As a reviewer, I want to understand the project quickly.
- The README covers scope, matching, data structures, determinism,
  invariants, testing, fuzzing, benchmarks and the explainer, with links.
