//! Line-based text encoding of commands, and a stable text form of events.
//!
//! One command per line; `#` starts a comment; blank lines are ignored.
//!
//! ```text
//! submit buy limit 100 5     # side, type, [price], quantity
//! submit sell ioc 99 3
//! submit buy market 7
//! cancel 3
//! amend 3 - 4                # id, new price or '-' to keep, new open quantity
//! amend 3 101 4
//! ```
//!
//! Numbers are parsed at full width (`i64` prices, `u64` quantities) so that
//! out-of-range values reach the engine and are rejected there, the same way
//! they would be from any other source.

use std::fmt;

use crate::command::{CancelReason, Command, Event, Priority, RejectReason};
use crate::types::{OrderId, OrderType, Price, Qty, Side};

/// What was wrong with a journal line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The first word is not a command.
    UnknownCommand(String),
    /// Expected `buy` or `sell`.
    UnknownSide(String),
    /// Expected `limit`, `ioc` or `market`.
    UnknownOrderType(String),
    /// A field was missing.
    Missing(&'static str),
    /// A field was not a number of the expected type.
    BadNumber {
        /// Field name.
        field: &'static str,
        /// Text found.
        text: String,
    },
    /// Extra text after a complete command.
    Trailing(String),
}

/// A journal parse failure with its 1-based line number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Line number, starting at 1.
    pub line: usize,
    /// What went wrong.
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(w) => write!(f, "unknown command `{w}`"),
            Self::UnknownSide(w) => write!(f, "expected `buy` or `sell`, found `{w}`"),
            Self::UnknownOrderType(w) => {
                write!(f, "expected `limit`, `ioc` or `market`, found `{w}`")
            }
            Self::Missing(field) => write!(f, "missing {field}"),
            Self::BadNumber { field, text } => write!(f, "invalid {field} `{text}`"),
            Self::Trailing(w) => write!(f, "unexpected `{w}` after command"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.kind)
    }
}

impl std::error::Error for ParseError {}

/// Parses one line. Returns `Ok(None)` for blank and comment-only lines.
///
/// # Errors
///
/// Returns the first problem found on the line.
pub fn parse_line(line: &str) -> Result<Option<Command>, ParseErrorKind> {
    let content = line.split_once('#').map_or(line, |(before, _)| before);
    let mut words = content.split_ascii_whitespace();
    let Some(verb) = words.next() else {
        return Ok(None);
    };
    let cmd = match verb {
        "submit" => {
            let side = match need(&mut words, "side")? {
                "buy" => Side::Buy,
                "sell" => Side::Sell,
                other => return Err(ParseErrorKind::UnknownSide(other.to_owned())),
            };
            let kind = match need(&mut words, "order type")? {
                "limit" => OrderType::Limit {
                    price: Price(number(&mut words, "price")?),
                },
                "ioc" => OrderType::Ioc {
                    price: Price(number(&mut words, "price")?),
                },
                "market" => OrderType::Market,
                other => return Err(ParseErrorKind::UnknownOrderType(other.to_owned())),
            };
            let qty = Qty(number(&mut words, "quantity")?);
            Command::Submit { side, kind, qty }
        }
        "cancel" => Command::Cancel {
            id: OrderId(number(&mut words, "order id")?),
        },
        "amend" => {
            let id = OrderId(number(&mut words, "order id")?);
            let price = match need(&mut words, "price")? {
                "-" => None,
                text => Some(Price(parse_num(text, "price")?)),
            };
            let qty = Qty(number(&mut words, "quantity")?);
            Command::Amend { id, price, qty }
        }
        other => return Err(ParseErrorKind::UnknownCommand(other.to_owned())),
    };
    match words.next() {
        Some(extra) => Err(ParseErrorKind::Trailing(extra.to_owned())),
        None => Ok(Some(cmd)),
    }
}

/// Parses a whole journal.
///
/// # Errors
///
/// Returns the first malformed line.
pub fn parse(text: &str) -> Result<Vec<Command>, ParseError> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        match parse_line(line) {
            Ok(Some(cmd)) => out.push(cmd),
            Ok(None) => {}
            Err(kind) => {
                return Err(ParseError {
                    line: i.saturating_add(1),
                    kind,
                });
            }
        }
    }
    Ok(out)
}

fn need<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
) -> Result<&'a str, ParseErrorKind> {
    words.next().ok_or(ParseErrorKind::Missing(field))
}

fn number<'a, T: std::str::FromStr>(
    words: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
) -> Result<T, ParseErrorKind> {
    parse_num(need(words, field)?, field)
}

fn parse_num<T: std::str::FromStr>(text: &str, field: &'static str) -> Result<T, ParseErrorKind> {
    text.parse().map_err(|_| ParseErrorKind::BadNumber {
        field,
        text: text.to_owned(),
    })
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        })
    }
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit { price } => write!(f, "limit {price}"),
            Self::Ioc { price } => write!(f, "ioc {price}"),
            Self::Market => f.write_str("market"),
        }
    }
}

/// Journal form; `parse_line(&cmd.to_string())` returns `cmd`.
impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Submit { side, kind, qty } => write!(f, "submit {side} {kind} {qty}"),
            Self::Cancel { id } => write!(f, "cancel {id}"),
            Self::Amend {
                id,
                price: Some(p),
                qty,
            } => write!(f, "amend {id} {p} {qty}"),
            Self::Amend {
                id,
                price: None,
                qty,
            } => write!(f, "amend {id} - {qty}"),
        }
    }
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroQuantity => "zero-quantity",
            Self::QuantityTooLarge => "quantity-too-large",
            Self::PriceOutOfRange => "price-out-of-range",
            Self::UnknownOrder => "unknown-order",
            Self::AmendNoChange => "amend-no-change",
        })
    }
}

impl fmt::Display for CancelReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "user",
            Self::IocRemainder => "ioc-remainder",
            Self::NoLiquidity => "no-liquidity",
        })
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Kept => "kept",
            Self::Lost => "lost",
        })
    }
}

/// Stable one-line form used by scenario files and the CLI.
impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Accepted {
                id,
                side,
                kind,
                qty,
                seq,
            } => write!(f, "accepted id={id} {side} {kind} qty={qty} seq={}", seq.0),
            Self::Rejected {
                id: Some(id),
                reason,
            } => write!(f, "rejected id={id} {reason}"),
            Self::Rejected { id: None, reason } => write!(f, "rejected {reason}"),
            Self::Amended {
                id,
                price,
                old_qty,
                qty,
                seq,
                priority,
            } => write!(
                f,
                "amended id={id} px={price} qty={old_qty}->{qty} seq={} {priority}",
                seq.0
            ),
            Self::Trade {
                maker,
                taker,
                taker_side,
                price,
                qty,
            } => write!(
                f,
                "trade maker={maker} taker={taker} {taker_side} px={price} qty={qty}"
            ),
            Self::Filled { id } => write!(f, "filled id={id}"),
            Self::Rested {
                id,
                side,
                price,
                qty,
                seq,
            } => write!(
                f,
                "rested id={id} {side} px={price} qty={qty} seq={}",
                seq.0
            ),
            Self::Cancelled { id, qty, reason } => {
                write!(f, "cancelled id={id} qty={qty} {reason}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Seq;

    fn cmd(line: &str) -> Command {
        parse_line(line).unwrap().unwrap()
    }

    #[test]
    fn parses_every_command_form() {
        assert_eq!(
            cmd("submit buy limit 100 5"),
            Command::Submit {
                side: Side::Buy,
                kind: OrderType::Limit { price: Price(100) },
                qty: Qty(5),
            }
        );
        assert_eq!(
            cmd("  submit sell ioc 99 3  # trailing comment"),
            Command::Submit {
                side: Side::Sell,
                kind: OrderType::Ioc { price: Price(99) },
                qty: Qty(3),
            }
        );
        assert_eq!(
            cmd("submit buy market 7"),
            Command::Submit {
                side: Side::Buy,
                kind: OrderType::Market,
                qty: Qty(7),
            }
        );
        assert_eq!(cmd("cancel 3"), Command::Cancel { id: OrderId(3) });
        assert_eq!(
            cmd("amend 3 - 4"),
            Command::Amend {
                id: OrderId(3),
                price: None,
                qty: Qty(4),
            }
        );
        assert_eq!(
            cmd("amend 3 101 4"),
            Command::Amend {
                id: OrderId(3),
                price: Some(Price(101)),
                qty: Qty(4),
            }
        );
    }

    #[test]
    fn out_of_range_values_parse_so_the_engine_can_reject_them() {
        assert_eq!(
            cmd("submit buy limit -5 0"),
            Command::Submit {
                side: Side::Buy,
                kind: OrderType::Limit { price: Price(-5) },
                qty: Qty(0),
            }
        );
    }

    #[test]
    fn blank_and_comment_lines_are_skipped() {
        assert_eq!(parse_line(""), Ok(None));
        assert_eq!(parse_line("   \t"), Ok(None));
        assert_eq!(parse_line("# submit buy limit 1 1"), Ok(None));
    }

    #[test]
    fn malformed_lines_report_what_is_wrong() {
        use ParseErrorKind as K;
        let err = |s| parse_line(s).unwrap_err();
        assert_eq!(err("buy 5"), K::UnknownCommand("buy".into()));
        assert_eq!(err("submit"), K::Missing("side"));
        assert_eq!(err("submit hold limit 1 1"), K::UnknownSide("hold".into()));
        assert_eq!(
            err("submit buy stop 1 1"),
            K::UnknownOrderType("stop".into())
        );
        assert_eq!(err("submit buy limit 100"), K::Missing("quantity"));
        assert_eq!(
            err("submit buy market 1.5"),
            K::BadNumber {
                field: "quantity",
                text: "1.5".into()
            }
        );
        assert_eq!(
            err("submit buy market -1"),
            K::BadNumber {
                field: "quantity",
                text: "-1".into()
            }
        );
        assert_eq!(err("cancel"), K::Missing("order id"));
        assert_eq!(err("cancel 1 2"), K::Trailing("2".into()));
        assert_eq!(
            err("amend 1 x 2"),
            K::BadNumber {
                field: "price",
                text: "x".into()
            }
        );
        assert_eq!(
            err("submit buy limit 99999999999999999999 1"),
            K::BadNumber {
                field: "price",
                text: "99999999999999999999".into()
            }
        );
    }

    #[test]
    fn journal_errors_carry_line_numbers() {
        let e = parse("submit buy limit 1 1\n\ncancel x\n").unwrap_err();
        assert_eq!(e.line, 3);
        assert_eq!(e.to_string(), "line 3: invalid order id `x`");
    }

    #[test]
    fn command_display_round_trips() {
        let text = "submit buy limit 100 5\nsubmit sell ioc 1 2\nsubmit sell market 9\n\
                    cancel 4\namend 2 - 3\namend 2 7 3\n";
        let cmds = parse(text).unwrap();
        let printed: String = cmds.iter().map(|c| format!("{c}\n")).collect();
        assert_eq!(printed, text);
        assert_eq!(parse(&printed).unwrap(), cmds);
    }

    #[test]
    fn event_text_is_stable() {
        let lines = [
            Event::Accepted {
                id: OrderId(1),
                side: Side::Buy,
                kind: OrderType::Limit { price: Price(100) },
                qty: Qty(5),
                seq: Seq(1),
            },
            Event::Rejected {
                id: None,
                reason: RejectReason::ZeroQuantity,
            },
            Event::Amended {
                id: OrderId(3),
                price: Price(101),
                old_qty: Qty(5),
                qty: Qty(7),
                seq: Seq(4),
                priority: Priority::Lost,
            },
            Event::Trade {
                maker: OrderId(1),
                taker: OrderId(4),
                taker_side: Side::Sell,
                price: Price(100),
                qty: Qty(2),
            },
            Event::Cancelled {
                id: OrderId(4),
                qty: Qty(3),
                reason: CancelReason::IocRemainder,
            },
        ]
        .map(|e| e.to_string());
        assert_eq!(
            lines,
            [
                "accepted id=1 buy limit 100 qty=5 seq=1",
                "rejected zero-quantity",
                "amended id=3 px=101 qty=5->7 seq=4 lost",
                "trade maker=1 taker=4 sell px=100 qty=2",
                "cancelled id=4 qty=3 ioc-remainder",
            ]
        );
    }
}
