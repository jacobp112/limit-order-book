// Step-through inspector for the lob engine (lob.wasm).
//
// Each journal line is sent to the engine as a whole; the events it returns
// are then applied to the displayed book one at a time (model.mjs), so the
// matching can be followed event by event. After the last event of a command
// the displayed book is compared with the engine's own book.

import { loadEngine } from "./lob.mjs";
import { applyEvent, canonical, clone, locate } from "./model.mjs";

const $ = (id) => document.getElementById(id);

const examples = {
  "price priority": `# asks at three prices; the buy fills 101 before 102 and never reaches 103
submit sell limit 103 3
submit sell limit 102 4
submit sell limit 101 5
submit buy limit 99 4
submit buy limit 103 7`,
  "time priority (FIFO)": `# three asks at 101 in arrival order 1, 2, 3
submit sell limit 101 4
submit sell limit 101 3
submit sell limit 101 5
submit buy limit 101 6`,
  "partial fill": `# the maker at the head of 101 is partially filled and keeps its place
submit sell limit 101 20
submit sell limit 101 4
submit buy limit 101 5
submit buy limit 101 5`,
  "multi-level sweep": `submit sell limit 101 2
submit sell limit 101 2
submit sell limit 102 3
submit sell limit 103 2
submit sell limit 103 3
submit sell limit 104 4
submit buy market 11`,
  "ioc and market remainders": `submit sell limit 101 3
submit sell limit 102 4
submit buy ioc 101 6      # 3 trade at 101, 3 cancelled
submit buy market 10      # 4 trade at 102, 6 cancelled`,
  "amend priority": `submit buy limit 100 5
submit buy limit 100 5
submit buy limit 100 5
amend 1 - 3               # size down: keeps its place
amend 2 - 8               # size up: moves behind 3
submit sell limit 100 9`,
  "session (examples/journals)": `submit sell limit 101 5
submit sell limit 102 5
submit sell limit 101 3
submit buy limit 99 4
submit buy limit 100 2
submit buy limit 101 7
amend 4 - 2
amend 5 100 6
submit sell ioc 99 10
submit buy market 3
cancel 2
cancel 2`,
};

let engine;
let model = { bids: [], asks: [] };
let cursor = 0; // index of the next journal line to execute
let pending = null; // { events, i, book, items }
let taker = null;
let timer = null;

const journal = $("journal");
const eventsList = $("events");

// ---- journal ----

function nextLine() {
  const lines = journal.value.split("\n");
  while (cursor < lines.length) {
    const i = cursor++;
    const command = lines[i].split("#")[0].trim();
    if (command) return { command, number: i + 1 };
  }
  return null;
}

function showPosition() {
  const total = journal.value.split("\n").length;
  $("position").textContent = cursor >= total && !pending ? "end of journal" : `next line ${cursor + 1}`;
}

// ---- stepping ----

function startCommand() {
  const next = nextLine();
  if (!next) return false;
  const out = engine.apply(next.command);
  if (!out.ok) {
    $("error").textContent = `line ${next.number}: ${out.error}`;
    $("error").hidden = false;
    stop();
    return false;
  }
  $("error").hidden = true;
  const head = document.createElement("li");
  head.className = "cmd";
  head.textContent = `> ${out.command}   (line ${next.number})`;
  eventsList.append(head);
  const items = out.events.map((e) => {
    const li = document.createElement("li");
    li.className = "pending";
    li.textContent = e.text;
    eventsList.append(li);
    return li;
  });
  eventsList.scrollTop = eventsList.scrollHeight;
  pending = { events: out.events, i: 0, book: out.book, items };
  return true;
}

function step() {
  if (!pending && !startCommand()) {
    render(model, {});
    showPosition();
    return false;
  }
  const e = pending.events[pending.i];
  eventsList.querySelectorAll(".current").forEach((el) => el.classList.remove("current"));
  const li = pending.items[pending.i];
  li.classList.remove("pending");
  li.classList.add("current");
  // Scroll only the event list, not the page.
  if (li.offsetTop < eventsList.scrollTop || li.offsetTop > eventsList.scrollTop + eventsList.clientHeight - li.offsetHeight) {
    eventsList.scrollTop = li.offsetTop - eventsList.clientHeight / 2;
  }

  // Removals are drawn struck through for one step before they disappear.
  const before = clone(model);
  let marks = {};
  switch (e.type) {
    case "accepted":
      taker = { ...e, left: e.qty };
      break;
    case "amended":
      if (e.priority === "lost") {
        marks = { gone: e.id };
        const at = locate(model, e.id);
        taker = { id: e.id, side: at?.side, orderType: "limit", price: e.price, qty: e.qty, left: e.qty, amend: true };
      } else {
        marks = { hit: e.id };
      }
      break;
    case "trade":
      marks = { hit: e.maker };
      if (taker) taker.left -= e.qty;
      addTrade(e);
      break;
    case "filled":
      if (taker && taker.id === e.id) taker.done = "filled";
      else marks = { gone: e.id };
      break;
    case "rested":
      marks = { new: e.id };
      if (taker) taker.done = `rested ${e.qty} at ${e.price}`;
      break;
    case "cancelled":
      if (e.reason === "user") marks = { gone: e.id };
      else if (taker) taker.done = `${e.qty} cancelled (${e.reason})`;
      break;
    case "rejected":
      taker = { rejected: e.reason };
      break;
  }
  applyEvent(model, e);
  render(marks.gone ? before : model, marks);
  showIncoming();

  pending.i++;
  if (pending.i === pending.events.length) finishCommand();
  showPosition();
  return true;
}

function finishCommand() {
  if (canonical(model) !== canonical(pending.book)) {
    // Should not happen (explainer/check.mjs tests it); the engine wins.
    console.warn("displayed book diverged from engine book", model, pending.book);
    model = clone(pending.book);
  }
  pending = null;
  $("digest").textContent = "0x" + BigInt.asUintN(64, engine.stateDigest()).toString(16).padStart(16, "0");
}

function nextCommand() {
  stop();
  if (!pending && !startCommand()) {
    showPosition();
    return;
  }
  while (pending) step();
  render(model, {});
}

function run() {
  if (timer) return stop();
  $("run").textContent = "Stop";
  timer = setInterval(() => {
    if (!step()) stop();
  }, 450);
}

function stop() {
  if (timer) clearInterval(timer);
  timer = null;
  $("run").textContent = "Run";
}

function resetBook() {
  stop();
  engine.reset();
  model = { bids: [], asks: [] };
  cursor = 0;
  pending = null;
  taker = null;
  eventsList.replaceChildren();
  $("trades").replaceChildren();
  $("error").hidden = true;
  $("digest").textContent = "0x" + BigInt.asUintN(64, engine.stateDigest()).toString(16).padStart(16, "0");
  render(model, {});
  showIncoming();
  showPosition();
}

// ---- drawing ----

function render(book, marks) {
  const body = $("book").tBodies[0];
  body.replaceChildren();
  const addRow = (side, level) => {
    const tr = body.insertRow();
    tr.className = side === "buy" ? "bid" : "ask";
    const size = level.orders.reduce((s, o) => s + o.qty, 0);
    tr.insertCell().textContent = side === "buy" ? "bid" : "ask";
    const p = tr.insertCell();
    p.className = "num";
    p.textContent = level.price;
    const s = tr.insertCell();
    s.className = "num";
    s.textContent = size;
    const q = tr.insertCell();
    for (const o of level.orders) {
      const span = document.createElement("span");
      span.className = "o";
      if (marks.hit === o.id) span.classList.add("hit");
      if (marks.new === o.id) span.classList.add("new");
      if (marks.gone === o.id) span.classList.add("gone");
      span.textContent = `#${o.id}:${o.qty}`;
      q.append(span);
    }
  };
  book.asks.slice().reverse().forEach((l) => addRow("sell", l));
  const bid = book.bids[0]?.price;
  const ask = book.asks[0]?.price;
  const sp = body.insertRow();
  sp.className = "spread";
  const cell = sp.insertCell();
  cell.colSpan = 4;
  cell.textContent =
    bid === undefined && ask === undefined ? "(empty book)" :
    bid === undefined || ask === undefined ? "spread: one side empty" : `spread ${ask - bid}`;
  book.bids.forEach((l) => addRow("buy", l));
}

function showIncoming() {
  const el = $("incoming");
  if (!taker) {
    el.textContent = "no command in progress";
    return;
  }
  if (taker.rejected) {
    el.textContent = `rejected: ${taker.rejected}`;
    return;
  }
  const what = taker.orderType === "market" ? "market" : `${taker.orderType} ${taker.price}`;
  const head = `${taker.amend ? "amended" : "incoming"} #${taker.id} ${taker.side ?? ""} ${what} qty ${taker.qty}`;
  el.textContent = taker.done ? `${head} -> ${taker.done}` : `${head}, ${taker.left} left to match`;
}

function addTrade(t) {
  const tr = $("trades").insertRow(0);
  for (const v of [t.price, t.qty, `#${t.maker}`, `#${t.taker} (${t.takerSide})`]) {
    tr.insertCell().textContent = v;
  }
  tr.cells[0].className = "num";
  tr.cells[1].className = "num";
}

// ---- wiring ----

for (const name of Object.keys(examples)) {
  $("example").add(new Option(name, name));
}
$("load").addEventListener("click", () => {
  journal.value = examples[$("example").value] + "\n";
  resetBook();
});
$("step").addEventListener("click", () => {
  stop();
  step();
});
$("next").addEventListener("click", nextCommand);
$("run").addEventListener("click", run);
$("reset").addEventListener("click", resetBook);
journal.addEventListener("input", showPosition);

function append(line) {
  const text = journal.value.replace(/\n*$/, "");
  journal.value = (text ? text + "\n" : "") + line + "\n";
  run();
}

$("builder").addEventListener("submit", (ev) => {
  ev.preventDefault();
  const f = ev.target.elements;
  const price = f.kind.value === "market" ? "" : ` ${f.price.value}`;
  append(`submit ${f.side.value} ${f.kind.value}${price} ${f.qty.value}`);
});
$("builder").elements.kind.addEventListener("change", (ev) => {
  $("builder").elements.price.disabled = ev.target.value === "market";
});
$("direct").addEventListener("submit", (ev) => {
  ev.preventDefault();
  const line = ev.target.elements.line.value.trim();
  if (line) append(line);
  ev.target.reset();
});

try {
  const bytes = await (await fetch(new URL("./lob.wasm", import.meta.url))).arrayBuffer();
  engine = await loadEngine(bytes);
  journal.value = examples["price priority"] + "\n";
  resetBook();
} catch (err) {
  $("error").textContent = `could not load lob.wasm (${err.message}); serve this directory over HTTP`;
  $("error").hidden = false;
}
