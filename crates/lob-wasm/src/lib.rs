//! WebAssembly exports so the visual explainer runs the real engine.
//!
//! The interface is deliberately small and text-based. JavaScript writes a
//! journal line (the same syntax `lob replay` reads) into an input buffer
//! owned by this module, calls [`lob_apply`], and reads a JSON document with
//! the resulting events and the whole book from the output buffer:
//!
//! ```text
//! ptr = lob_input(len); write UTF-8 bytes at ptr; lob_apply(len);
//! read lob_output_len() bytes at lob_output_ptr()
//! ```
//!
//! Output shape: `{"ok":true,"events":[...],"book":{...}}` or
//! `{"ok":false,"error":"..."}`. Each event carries its stable text form in
//! `"text"`, identical to what `lob replay` prints.
//!
//! The module holds one engine. `wasm32-unknown-unknown` is single-threaded;
//! the `Mutex` only exists so the statics are safe Rust.

use std::fmt::Write as _;
use std::sync::Mutex;

use lob::snapshot::fnv1a;
use lob::{Event, MatchingEngine, OrderBook, Side, journal};

struct State {
    engine: MatchingEngine,
    input: Vec<u8>,
    output: Vec<u8>,
    /// Stable text of every event so far, for the event digest.
    event_log: String,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = guard.get_or_insert_with(|| State {
        engine: MatchingEngine::new(),
        input: Vec::new(),
        output: Vec::new(),
        event_log: String::new(),
    });
    f(state)
}

/// Discards the book and starts again with an empty one (ids restart at 1).
#[unsafe(no_mangle)]
pub extern "C" fn lob_reset() {
    with_state(|s| {
        s.engine = MatchingEngine::new();
        s.event_log.clear();
        s.output = book_only(s.engine.book()).into_bytes();
    });
}

/// Returns a buffer of `len` bytes for the caller to fill with one journal
/// line. The pointer is valid until the next call into this module.
#[unsafe(no_mangle)]
pub extern "C" fn lob_input(len: usize) -> *mut u8 {
    with_state(|s| {
        s.input.clear();
        s.input.resize(len, 0);
        s.input.as_mut_ptr()
    })
}

/// Parses the first `len` input bytes as one journal line and applies it.
/// Returns 1 if a command was applied, 0 otherwise (parse error or blank).
#[unsafe(no_mangle)]
pub extern "C" fn lob_apply(len: usize) -> u32 {
    with_state(|s| {
        let end = len.min(s.input.len());
        let line = String::from_utf8_lossy(&s.input[..end]).into_owned();
        let (json, applied) = apply_line(s, &line);
        s.output = json.into_bytes();
        u32::from(applied)
    })
}

/// Writes the current book to the output buffer without applying anything.
#[unsafe(no_mangle)]
pub extern "C" fn lob_book() {
    with_state(|s| s.output = book_only(s.engine.book()).into_bytes());
}

/// Pointer to the last output document.
#[unsafe(no_mangle)]
pub extern "C" fn lob_output_ptr() -> *const u8 {
    with_state(|s| s.output.as_ptr())
}

/// Length in bytes of the last output document.
#[unsafe(no_mangle)]
pub extern "C" fn lob_output_len() -> usize {
    with_state(|s| s.output.len())
}

/// FNV-1a digest of the canonical snapshot, as printed by `lob replay`.
#[unsafe(no_mangle)]
pub extern "C" fn lob_state_digest() -> u64 {
    with_state(|s| s.engine.snapshot().digest())
}

/// FNV-1a digest of all event text so far, as printed by `lob replay`.
#[unsafe(no_mangle)]
pub extern "C" fn lob_event_digest() -> u64 {
    with_state(|s| fnv1a(s.event_log.as_bytes()))
}

fn apply_line(s: &mut State, line: &str) -> (String, bool) {
    let cmd = match journal::parse_line(line) {
        Ok(Some(cmd)) => cmd,
        Ok(None) => return (error("empty command"), false),
        Err(e) => return (error(&e.to_string()), false),
    };
    let mut events = Vec::new();
    s.engine.apply(cmd, &mut events);
    for e in &events {
        writeln!(s.event_log, "{e}").expect("writing to a String cannot fail");
    }
    let mut out = String::from("{\"ok\":true,\"command\":");
    string(&mut out, &cmd.to_string());
    out.push_str(",\"events\":[");
    for (i, e) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        event(&mut out, e);
    }
    out.push_str("],\"book\":");
    book(&mut out, s.engine.book());
    out.push('}');
    (out, true)
}

fn error(message: &str) -> String {
    let mut out = String::from("{\"ok\":false,\"error\":");
    string(&mut out, message);
    out.push('}');
    out
}

fn book_only(b: &OrderBook) -> String {
    let mut out = String::from("{\"ok\":true,\"events\":[],\"book\":");
    book(&mut out, b);
    out.push('}');
    out
}

fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(c)).expect("writing to a String cannot fail");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Appends `"key":value` pairs; values are already JSON.
fn fields(out: &mut String, pairs: &[(&str, String)]) {
    for (k, v) in pairs {
        write!(out, ",\"{k}\":{v}").expect("writing to a String cannot fail");
    }
}

fn quoted(s: impl std::fmt::Display) -> String {
    let mut out = String::new();
    string(&mut out, &s.to_string());
    out
}

fn event(out: &mut String, e: &Event) {
    let (kind, pairs): (&str, Vec<(&str, String)>) = match *e {
        Event::Accepted {
            id,
            side,
            kind,
            qty,
            seq,
        } => (
            "accepted",
            vec![
                ("id", id.to_string()),
                ("side", quoted(side)),
                (
                    "orderType",
                    quoted(match kind {
                        lob::OrderType::Limit { .. } => "limit",
                        lob::OrderType::Ioc { .. } => "ioc",
                        lob::OrderType::Market => "market",
                    }),
                ),
                (
                    "price",
                    kind.limit().map_or("null".into(), |p| p.to_string()),
                ),
                ("qty", qty.to_string()),
                ("seq", seq.0.to_string()),
            ],
        ),
        Event::Rejected { id, reason } => (
            "rejected",
            vec![
                ("id", id.map_or("null".into(), |id| id.to_string())),
                ("reason", quoted(reason)),
            ],
        ),
        Event::Amended {
            id,
            price,
            old_qty,
            qty,
            seq,
            priority,
        } => (
            "amended",
            vec![
                ("id", id.to_string()),
                ("price", price.to_string()),
                ("oldQty", old_qty.to_string()),
                ("qty", qty.to_string()),
                ("seq", seq.0.to_string()),
                ("priority", quoted(priority)),
            ],
        ),
        Event::Trade {
            maker,
            taker,
            taker_side,
            price,
            qty,
        } => (
            "trade",
            vec![
                ("maker", maker.to_string()),
                ("taker", taker.to_string()),
                ("takerSide", quoted(taker_side)),
                ("price", price.to_string()),
                ("qty", qty.to_string()),
            ],
        ),
        Event::Filled { id } => ("filled", vec![("id", id.to_string())]),
        Event::Rested {
            id,
            side,
            price,
            qty,
            seq,
        } => (
            "rested",
            vec![
                ("id", id.to_string()),
                ("side", quoted(side)),
                ("price", price.to_string()),
                ("qty", qty.to_string()),
                ("seq", seq.0.to_string()),
            ],
        ),
        Event::Cancelled { id, qty, reason } => (
            "cancelled",
            vec![
                ("id", id.to_string()),
                ("qty", qty.to_string()),
                ("reason", quoted(reason)),
            ],
        ),
    };
    out.push_str("{\"type\":");
    string(out, kind);
    fields(out, &pairs);
    out.push_str(",\"text\":");
    string(out, &e.to_string());
    out.push('}');
}

/// Both sides best price first; each level lists orders in queue order.
fn book(out: &mut String, b: &OrderBook) {
    out.push('{');
    for (i, (side, key)) in [(Side::Buy, "bids"), (Side::Sell, "asks")]
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            out.push(',');
        }
        write!(out, "\"{key}\":[").expect("writing to a String cannot fail");
        for (j, level) in b.depth(side, usize::MAX).iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            write!(
                out,
                "{{\"price\":{},\"qty\":{},\"orders\":[",
                level.price, level.qty
            )
            .expect("writing to a String cannot fail");
            for (k, o) in b.level_orders(side, level.price).iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write!(
                    out,
                    "{{\"id\":{},\"qty\":{},\"seq\":{}}}",
                    o.id, o.qty, o.seq.0
                )
                .expect("writing to a String cannot fail");
            }
            out.push_str("]}");
        }
        out.push(']');
    }
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> State {
        State {
            engine: MatchingEngine::new(),
            input: Vec::new(),
            output: Vec::new(),
            event_log: String::new(),
        }
    }

    #[test]
    fn applies_a_line_and_reports_events_and_book() {
        let mut s = fresh();
        let (json, ok) = apply_line(&mut s, "submit sell limit 101 5");
        assert!(ok);
        assert_eq!(
            json,
            "{\"ok\":true,\"command\":\"submit sell limit 101 5\",\"events\":[\
             {\"type\":\"accepted\",\"id\":1,\"side\":\"sell\",\"orderType\":\"limit\",\"price\":101,\
             \"qty\":5,\"seq\":1,\"text\":\"accepted id=1 sell limit 101 qty=5 seq=1\"},\
             {\"type\":\"rested\",\"id\":1,\"side\":\"sell\",\"price\":101,\"qty\":5,\"seq\":1,\
             \"text\":\"rested id=1 sell px=101 qty=5 seq=1\"}],\
             \"book\":{\"bids\":[],\"asks\":[{\"price\":101,\"qty\":5,\"orders\":[{\"id\":1,\"qty\":5,\"seq\":1}]}]}}"
        );
    }

    #[test]
    fn trades_and_market_orders_serialise() {
        let mut s = fresh();
        apply_line(&mut s, "submit sell limit 101 5");
        let (json, _) = apply_line(&mut s, "submit buy market 7");
        assert!(json.contains(
            "{\"type\":\"trade\",\"maker\":1,\"taker\":2,\"takerSide\":\"buy\",\"price\":101,\"qty\":5,"
        ));
        assert!(json.contains("\"orderType\":\"market\",\"price\":null"));
        assert!(json.contains("\"reason\":\"no-liquidity\""));
        assert!(json.ends_with("\"book\":{\"bids\":[],\"asks\":[]}}"));
    }

    #[test]
    fn parse_errors_are_reported_not_applied() {
        let mut s = fresh();
        let (json, ok) = apply_line(&mut s, "submit sideways limit 1 1");
        assert!(!ok);
        assert_eq!(
            json,
            "{\"ok\":false,\"error\":\"expected `buy` or `sell`, found `sideways`\"}"
        );
        let (json, ok) = apply_line(&mut s, "cancel \"x\"");
        assert!(!ok);
        assert!(json.contains("invalid order id `\\\"x\\\"`"));
        assert_eq!(s.engine.snapshot(), MatchingEngine::new().snapshot());
    }

    #[test]
    fn digests_match_the_cli_for_the_example_session() {
        // Same journal and digests as `lob replay examples/journals/session.journal`.
        let text = include_str!("../../../examples/journals/session.journal");
        let mut s = fresh();
        for line in text.lines() {
            if journal::parse_line(line).ok().flatten().is_some() {
                assert!(apply_line(&mut s, line).1);
            }
        }
        assert_eq!(s.engine.snapshot().digest(), 0x99d8_87fd_8c46_ef4e);
        assert_eq!(fnv1a(s.event_log.as_bytes()), 0x3992_e01f_9196_0e21);
    }
}
