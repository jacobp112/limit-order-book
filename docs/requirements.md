# Requirements

## Scope

A single-instrument, in-memory limit order book with a deterministic matching
engine. Orders match by **price priority**, then **time (FIFO) priority**
within a price level.

In scope:

- limit, market and immediate-or-cancel (IOC) orders
- cancellation and amendment of resting orders
- partial fills and sweeps across multiple price levels
- a command journal that can be replayed to reproduce state and output exactly
- a canonical snapshot of book state, with restore

Out of scope, deliberately:

- networking, FIX, sessions, authentication
- persistence beyond the command journal and snapshots
- multiple instruments, fees, risk checks, self-trade prevention
- stop, fill-or-kill, iceberg and pegged orders
- wall-clock timestamps inside the engine

This is a study of matching-engine mechanics, not an exchange.

## Units

| Quantity | Type | Unit | Valid range |
|---|---|---|---|
| Price | `i64` newtype | ticks | `1 ..= MAX_PRICE` |
| Quantity | `u64` newtype | lots | `1 ..= MAX_QTY` |
| Order id | `u64` newtype | assigned by engine, starts at 1 | – |
| Sequence | `u64` newtype | logical clock, one tick per accepted book change | – |

No floating point is used for price or quantity. The bounds are chosen so that
the total quantity of any price level, and any ledger sum used in tests, fits in
the accumulator type without overflow.

## Commands

| Command | Fields |
|---|---|
| `Submit` | side, order type, quantity |
| `Cancel` | order id |
| `Amend` | order id, new price (optional), new open quantity |

Order types: `Limit { price }`, `Ioc { price }`, `Market`.

## Order semantics

- **Limit.** Matches against the opposite side while it crosses. Each trade
  executes at the resting (maker) order's price. Any remainder rests at the
  limit price at the back of that level's queue.
- **IOC.** As limit, but any remainder is cancelled instead of resting.
- **Market.** No price. Consumes opposite liquidity until filled or the
  opposite side is empty. Any remainder is cancelled. Never rests.
- **Cancel.** Removes a resting order. Reports the open quantity removed.
- **Amend.** Applies to a resting order and sets its new *open* quantity,
  optionally with a new price.
  - same price, lower quantity: keeps its queue position
  - same price, higher quantity: moves to the back of the queue
  - different price: moves to the back of the queue at the new price, and if
    the new price crosses it executes immediately as an aggressor; any
    remainder rests
  - new quantity of zero: rejected (cancel is explicit)
  - no effective change: rejected

A resting order's queue position is represented by its sequence number. Losing
priority means receiving a new sequence number.

## Error semantics

Every command is validated before any state changes. An invalid command
produces exactly one `Rejected { reason }` event and leaves the book unchanged,
including the id and sequence counters.

| Reason | When |
|---|---|
| `ZeroQuantity` | quantity is 0 |
| `QuantityTooLarge` | quantity > `MAX_QTY` |
| `PriceOutOfRange` | price < 1 or > `MAX_PRICE` |
| `UnknownOrder` | id was never issued, or the order is no longer resting |
| `AmendNoChange` | amend leaves both price and quantity unchanged |

A panic indicates a violated internal invariant, i.e. a bug. It is never used
for bad input.

## Invariants

Notation: for an order *i*, `A_i` is its accepted quantity (original quantity
plus any amend increases), `F_i` filled quantity, `O_i` open (resting)
quantity, `C_i` cancelled quantity (user cancel, amend decreases, IOC and
market remainders). `T` is the multiset of trades.

**I1 — per-order conservation.** For every accepted order, at all times:
`A_i = F_i + O_i + C_i`.

**I2 — trade symmetry.** `Σ_{buy i} F_i = Σ_{sell i} F_i = Σ_{t ∈ T} qty_t`.

**I3 — rejection neutrality.** A rejected command changes no `A`, `F`, `O`, `C`
and emits no trade.

**I4 — level consistency.** For each price level `L`:
`L.total = Σ_{i ∈ L} O_i`, `L.count = |L|`, and `L.count > 0`.

**I5 — index consistency.** An order id is in the id index if and only if it
is in exactly one price level, on the side and at the price it records.

**I6 — positive open quantity.** Every resting order has `O_i > 0`.

**I7 — FIFO.** Within a level, sequence numbers strictly increase from head to
tail.

**I8 — uncrossed book.** After every command, if both sides are non-empty,
`best_bid < best_ask`.

**I9 — execution prices.** Every trade executes at the maker's resting price,
and never outside the taker's limit (for limit and IOC takers).

**I10 — priority.** Within one command, a taker's fills come from strictly
better or equal prices in order, and within a price in increasing sequence.

## Determinism

For an initial state `S` and command sequence `E`, `replay(S, E)` yields an
event sequence and a final state. Both must be identical on every run, on
every platform supported by CI.

Consequences for the implementation:

- no clock, randomness, thread scheduling or address-dependent behaviour on
  the matching path
- hash maps are used only for lookup and are never iterated to produce output
- the canonical snapshot is a byte string with a fixed field order

Snapshot/restore must compose with replay:
`replay(S, E1 ++ E2) == replay(restore(snapshot(replay(S, E1))), E2)`.
