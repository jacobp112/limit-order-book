//! `lob replay`: run a command journal and print events and state digests.

use std::fmt::Write as _;
use std::fs;
use std::process::ExitCode;

use lob::snapshot::fnv1a;
use lob::{Event, MatchingEngine, Snapshot, journal};

const USAGE: &str = "\
usage: lob replay <journal> [--quiet] [--from <snapshot>] [--save <snapshot>]

  --quiet            print only the summary and digests
  --from <file>      start from a saved snapshot instead of an empty book
  --save <file>      write the final snapshot to a file";

struct Args {
    journal: String,
    quiet: bool,
    from: Option<String>,
    save: Option<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    if args.next().as_deref() != Some("replay") {
        return Err(USAGE.to_owned());
    }
    let mut journal = None;
    let mut quiet = false;
    let mut from = None;
    let mut save = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quiet" => quiet = true,
            "--from" => from = Some(args.next().ok_or("--from needs a file")?),
            "--save" => save = Some(args.next().ok_or("--save needs a file")?),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ if journal.is_none() => journal = Some(arg),
            _ => return Err(USAGE.to_owned()),
        }
    }
    Ok(Args {
        journal: journal.ok_or(USAGE)?,
        quiet,
        from,
        save,
    })
}

fn run(args: &Args) -> Result<String, String> {
    let text = fs::read_to_string(&args.journal).map_err(|e| format!("{}: {e}", args.journal))?;
    let commands = journal::parse(&text).map_err(|e| format!("{}: {e}", args.journal))?;

    let mut engine = match &args.from {
        Some(path) => {
            let bytes = fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let snap = Snapshot::decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
            MatchingEngine::restore(&snap).map_err(|e| format!("{path}: {e}"))?
        }
        None => MatchingEngine::new(),
    };

    let mut out = String::new();
    let mut events = Vec::new();
    let mut event_text = String::new();
    for cmd in &commands {
        let start = events.len();
        engine.apply(*cmd, &mut events);
        if !args.quiet {
            writeln!(out, "> {cmd}").unwrap();
        }
        for e in &events[start..] {
            writeln!(event_text, "{e}").unwrap();
            if !args.quiet {
                writeln!(out, "  {e}").unwrap();
            }
        }
    }

    let snap = engine.snapshot();
    if let Some(path) = &args.save {
        fs::write(path, snap.encode()).map_err(|e| format!("{path}: {e}"))?;
    }
    let trades = events
        .iter()
        .filter(|e| matches!(e, Event::Trade { .. }))
        .count();
    let resting = snap.bids.len().saturating_add(snap.asks.len());
    writeln!(
        out,
        "commands {}  events {}  trades {trades}  resting {resting}",
        commands.len(),
        events.len()
    )
    .unwrap();
    writeln!(out, "event digest {:#018x}", fnv1a(event_text.as_bytes())).unwrap();
    writeln!(out, "state digest {:#018x}", snap.digest()).unwrap();
    Ok(out)
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
