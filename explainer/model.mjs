// The explainer's copy of the book, updated from engine events only.
//
// A book is { bids: [level], asks: [level] }, best price first; a level is
// { price, orders: [{ id, qty, seq }] } in queue order. `applyEvent` changes
// the book exactly as the event describes; check.mjs verifies after every
// command that the result equals the engine's own book.

export const clone = (book) => JSON.parse(JSON.stringify(book));
export const levels = (book, side) => (side === "buy" ? book.bids : book.asks);

export function locate(book, id) {
  for (const side of ["buy", "sell"]) {
    for (const level of levels(book, side)) {
      const index = level.orders.findIndex((o) => o.id === id);
      if (index >= 0) return { side, level, index, order: level.orders[index] };
    }
  }
  return null;
}

function remove(book, id) {
  const at = locate(book, id);
  if (!at) return;
  at.level.orders.splice(at.index, 1);
  const list = levels(book, at.side);
  if (at.level.orders.length === 0) list.splice(list.indexOf(at.level), 1);
}

function add(book, side, price, order) {
  const list = levels(book, side);
  let level = list.find((l) => l.price === price);
  if (!level) {
    level = { price, orders: [] };
    list.push(level);
    list.sort((a, b) => (side === "buy" ? b.price - a.price : a.price - b.price));
  }
  level.orders.push(order);
}

export function applyEvent(book, e) {
  switch (e.type) {
    case "amended":
      if (e.priority === "kept") {
        const at = locate(book, e.id);
        if (at) at.order.qty = e.qty;
      } else {
        remove(book, e.id); // re-enters via trades and/or `rested`
      }
      break;
    case "trade": {
      const at = locate(book, e.maker);
      if (at) at.order.qty -= e.qty;
      break;
    }
    case "filled": // a maker leaves; a taker was never in the book
    case "cancelled": // user cancel removes; IOC/market remainders never rested
      remove(book, e.id);
      break;
    case "rested":
      add(book, e.side, e.price, { id: e.id, qty: e.qty, seq: e.seq });
      break;
    default: // accepted, rejected: no change to resting orders
      break;
  }
}

export const canonical = (book) =>
  JSON.stringify(
    ["bids", "asks"].map((k) => book[k].map((l) => [l.price, l.orders.map((o) => [o.id, o.qty, o.seq])])),
  );
