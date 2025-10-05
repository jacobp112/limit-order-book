//! Runs every `tests/scenarios/*.scn` file.
//!
//! Format:
//!
//! ```text
//! # comment
//! > submit sell limit 100 2          command, in journal syntax
//! accepted id=1 sell limit 100 qty=2 seq=1
//! rested id=1 sell px=100 qty=2 seq=1
//! = ask 100: 1x2                     optional: the whole book afterwards
//! ```
//!
//! After each command, the event lines must match the emitted events exactly
//! and in order. If a block has any `=` lines, they must describe the entire
//! book as a ladder: asks from highest to lowest price, then bids from highest
//! to lowest, each level listing `idxqty` in queue order. `= empty` asserts an
//! empty book.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use lob::{MatchingEngine, Side, journal};

struct Block {
    line: usize,
    command: String,
    events: Vec<String>,
    book: Vec<String>,
}

fn parse_scenario(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for (n, raw) in (1..).zip(text.lines()) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(cmd) = line.strip_prefix('>') {
            blocks.push(Block {
                line: n,
                command: cmd.trim().to_owned(),
                events: Vec::new(),
                book: Vec::new(),
            });
            continue;
        }
        let block = blocks
            .last_mut()
            .unwrap_or_else(|| panic!("line {}: expectation before first command", n));
        match line.strip_prefix('=') {
            Some(level) => block.book.push(level.trim().to_owned()),
            None => block.events.push(line.to_owned()),
        }
    }
    blocks
}

fn ladder(engine: &MatchingEngine) -> Vec<String> {
    let book = engine.book();
    let level = |side: Side, label: &str, price| {
        let orders: Vec<_> = book
            .level_orders(side, price)
            .iter()
            .map(|o| format!("{}x{}", o.id, o.qty))
            .collect();
        format!("{label} {price}: {}", orders.join(" "))
    };
    let mut lines: Vec<String> = book
        .depth(Side::Sell, usize::MAX)
        .iter()
        .rev()
        .map(|l| level(Side::Sell, "ask", l.price))
        .collect();
    lines.extend(
        book.depth(Side::Buy, usize::MAX)
            .iter()
            .map(|l| level(Side::Buy, "bid", l.price)),
    );
    if lines.is_empty() {
        lines.push("empty".to_owned());
    }
    lines
}

fn run(path: &Path) -> Result<usize, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut engine = MatchingEngine::new();
    let mut journal_so_far = String::new();
    let blocks = parse_scenario(&text);
    for block in &blocks {
        let cmd = journal::parse_line(&block.command)
            .map_err(|e| format!("line {}: {e}", block.line))?
            .ok_or_else(|| format!("line {}: empty command", block.line))?;
        writeln!(journal_so_far, "{cmd}").unwrap();

        let mut out = Vec::new();
        engine.apply(cmd, &mut out);
        let events: Vec<String> = out.iter().map(ToString::to_string).collect();
        let book = ladder(&engine);

        let events_ok = events == block.events;
        let book_ok = block.book.is_empty() || book == block.book;
        if !(events_ok && book_ok) {
            let mut msg = format!("line {}: `{}`\n", block.line, block.command);
            if !events_ok {
                write!(
                    msg,
                    "  expected events:\n    {}\n  actual events:\n    {}\n",
                    block.events.join("\n    "),
                    events.join("\n    ")
                )
                .unwrap();
            }
            if !book_ok {
                write!(
                    msg,
                    "  expected book:\n    {}\n  actual book:\n    {}\n",
                    block.book.join("\n    "),
                    book.join("\n    ")
                )
                .unwrap();
            }
            write!(msg, "  journal to reproduce:\n{journal_so_far}").unwrap();
            return Err(msg);
        }
    }
    Ok(blocks.len())
}

#[test]
fn scenarios() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .expect("scenario directory")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "scn"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scenarios in {}", dir.display());

    let mut failures = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy();
        match run(path) {
            Ok(n) => println!("{name}: {n} commands ok"),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
