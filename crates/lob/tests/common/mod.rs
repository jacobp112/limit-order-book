//! Deterministic command-stream generator shared by integration tests.
//!
//! Uses a fixed-seed xorshift so streams are identical on every platform and
//! run, without pulling in a random-number dependency.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_wrap,
    reason = "test data generation with small bounded values, not engine accounting"
)]

use lob::{Command, MAX_PRICE, MAX_QTY, OrderId, OrderType, Price, Qty, Side};

pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub(crate) fn next(&mut self) -> u64 {
        // xorshift64
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform-ish in `0..n` (n > 0); modulo bias is irrelevant here.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// A mixed stream around a mid price of 1000: mostly valid limit orders,
/// plus market, IOC, cancels and amends that target recent ids, and a small
/// share of invalid commands.
pub(crate) fn mixed_stream(seed: u64, len: usize) -> Vec<Command> {
    let mut rng = Rng::new(seed);
    let mut issued = 0u64; // upper bound on ids the engine could have issued
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        let target = OrderId(if issued == 0 {
            1
        } else {
            issued - rng.below(issued.min(40))
        });
        let cmd = match rng.below(100) {
            0..=54 => {
                issued += 1;
                let side = side(&mut rng);
                let kind = OrderType::Limit {
                    price: near_mid(&mut rng, side),
                };
                Command::Submit {
                    side,
                    kind,
                    qty: qty(&mut rng),
                }
            }
            55..=62 => {
                issued += 1;
                let side = side(&mut rng);
                Command::Submit {
                    side,
                    kind: OrderType::Ioc {
                        price: near_mid(&mut rng, side),
                    },
                    qty: qty(&mut rng),
                }
            }
            63..=67 => {
                issued += 1;
                Command::Submit {
                    side: side(&mut rng),
                    kind: OrderType::Market,
                    qty: qty(&mut rng),
                }
            }
            68..=81 => Command::Cancel { id: target },
            82..=95 => Command::Amend {
                id: target,
                price: rng.chance(50).then(|| Price(990 + rng.below(21) as i64)),
                qty: qty(&mut rng),
            },
            _ => invalid(&mut rng),
        };
        out.push(cmd);
    }
    out
}

fn side(rng: &mut Rng) -> Side {
    if rng.chance(50) {
        Side::Buy
    } else {
        Side::Sell
    }
}

/// Prices cluster just behind the mid on the order's own side, with some
/// orders crossing.
fn near_mid(rng: &mut Rng, side: Side) -> Price {
    let offset = rng.below(12) as i64 - 3;
    Price(match side {
        Side::Buy => 1000 - offset,
        Side::Sell => 1000 + offset,
    })
}

fn qty(rng: &mut Rng) -> Qty {
    let max = if rng.chance(10) { 500 } else { 20 };
    Qty(1 + rng.below(max))
}

fn invalid(rng: &mut Rng) -> Command {
    let side = side(rng);
    match rng.below(4) {
        0 => Command::Submit {
            side,
            kind: OrderType::Limit { price: Price(1000) },
            qty: Qty(0),
        },
        1 => Command::Submit {
            side,
            kind: OrderType::Limit {
                price: Price(MAX_PRICE + 1),
            },
            qty: Qty(1),
        },
        2 => Command::Submit {
            side,
            kind: OrderType::Market,
            qty: Qty(MAX_QTY + 1),
        },
        _ => Command::Cancel {
            id: OrderId(u64::MAX),
        },
    }
}
