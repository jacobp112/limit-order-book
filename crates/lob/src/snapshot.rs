//! Canonical engine state: everything that affects future output.
//!
//! Two engines with equal snapshots produce identical events for any
//! command sequence. The byte encoding is fixed (little-endian, fixed field
//! order, orders in priority order), so equal states encode to equal bytes
//! on every platform.
//!
//! ```text
//! "LOBSNAP1" next_id:u64 next_seq:u64
//! n_bids:u64 { id:u64 price:i64 qty:u64 seq:u64 } * n_bids    best bid first
//! n_asks:u64 { id:u64 price:i64 qty:u64 seq:u64 } * n_asks    best ask first
//! ```

use std::collections::BTreeSet;
use std::fmt;

use crate::book::RestingOrder;
use crate::types::{OrderId, Price, Qty, Seq, Side};

const MAGIC: &[u8; 8] = b"LOBSNAP1";
const ORDER_BYTES: usize = 32;

/// Engine state in canonical form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Id the next accepted order will receive.
    pub next_id: u64,
    /// Sequence number the next accepted or re-queued order will receive.
    pub next_seq: u64,
    /// Resting bids, best price first, time priority within a price.
    pub bids: Vec<RestingOrder>,
    /// Resting asks, best price first, time priority within a price.
    pub asks: Vec<RestingOrder>,
}

/// Why bytes or a snapshot do not describe a valid engine state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// Missing or wrong header.
    BadMagic,
    /// Input ended early.
    Truncated,
    /// Input continued after the last order.
    TrailingBytes,
    /// `next_id` or `next_seq` is zero.
    ZeroCounter,
    /// An order is listed under the wrong side.
    WrongSide(OrderId),
    /// Price outside `1..=MAX_PRICE`.
    PriceOutOfRange(OrderId),
    /// Quantity outside `1..=MAX_QTY`.
    QuantityOutOfRange(OrderId),
    /// Id is zero or not below `next_id`.
    IdNotIssued(OrderId),
    /// Sequence number is zero or not below `next_seq`.
    SeqNotIssued(OrderId),
    /// Same id appears twice.
    DuplicateId(OrderId),
    /// Orders are not in price-time priority order.
    OutOfOrder(OrderId),
    /// Best bid is not below best ask.
    Crossed,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => f.write_str("not a snapshot (bad header)"),
            Self::Truncated => f.write_str("snapshot is truncated"),
            Self::TrailingBytes => f.write_str("unexpected bytes after snapshot"),
            Self::ZeroCounter => f.write_str("id and sequence counters must start at 1"),
            Self::WrongSide(id) => write!(f, "order {id} listed on the wrong side"),
            Self::PriceOutOfRange(id) => write!(f, "order {id} has an invalid price"),
            Self::QuantityOutOfRange(id) => write!(f, "order {id} has an invalid quantity"),
            Self::IdNotIssued(id) => write!(f, "order id {id} was never issued"),
            Self::SeqNotIssued(id) => write!(f, "order {id} has an unissued sequence number"),
            Self::DuplicateId(id) => write!(f, "order id {id} appears twice"),
            Self::OutOfOrder(id) => write!(f, "order {id} is out of priority order"),
            Self::Crossed => f.write_str("book is crossed"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl Snapshot {
    /// Canonical byte encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let orders = self.bids.len().saturating_add(self.asks.len());
        let mut out = Vec::with_capacity(ORDER_BYTES.saturating_mul(orders).saturating_add(40));
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.next_id.to_le_bytes());
        out.extend_from_slice(&self.next_seq.to_le_bytes());
        for side in [&self.bids, &self.asks] {
            out.extend_from_slice(&(side.len() as u64).to_le_bytes());
            for o in side {
                out.extend_from_slice(&o.id.0.to_le_bytes());
                out.extend_from_slice(&o.price.0.to_le_bytes());
                out.extend_from_slice(&o.qty.0.to_le_bytes());
                out.extend_from_slice(&o.seq.0.to_le_bytes());
            }
        }
        out
    }

    /// Decodes bytes produced by [`Snapshot::encode`] and validates them.
    ///
    /// # Errors
    ///
    /// Returns why the bytes are not a valid snapshot.
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let mut r = Reader(bytes);
        if r.take(MAGIC.len())? != MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let next_id = r.u64()?;
        let next_seq = r.u64()?;
        let bids = r.orders(Side::Buy)?;
        let asks = r.orders(Side::Sell)?;
        if !r.0.is_empty() {
            return Err(SnapshotError::TrailingBytes);
        }
        let snap = Self {
            next_id,
            next_seq,
            bids,
            asks,
        };
        snap.validate()?;
        Ok(snap)
    }

    /// 64-bit FNV-1a hash of the canonical encoding, for compact comparison.
    /// Not collision resistant against an adversary; tests compare full bytes.
    #[must_use]
    pub fn digest(&self) -> u64 {
        fnv1a(&self.encode())
    }

    /// Checks that the snapshot describes a state the engine could be in.
    ///
    /// # Errors
    ///
    /// Returns the first problem found.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        if self.next_id == 0 || self.next_seq == 0 {
            return Err(SnapshotError::ZeroCounter);
        }
        let mut ids = BTreeSet::new();
        for (side, orders) in [(Side::Buy, &self.bids), (Side::Sell, &self.asks)] {
            let mut prev: Option<&RestingOrder> = None;
            for o in orders {
                let id = o.id;
                if o.side != side {
                    return Err(SnapshotError::WrongSide(id));
                }
                if !o.price.is_valid() {
                    return Err(SnapshotError::PriceOutOfRange(id));
                }
                if !o.qty.is_valid() {
                    return Err(SnapshotError::QuantityOutOfRange(id));
                }
                if id.0 == 0 || id.0 >= self.next_id {
                    return Err(SnapshotError::IdNotIssued(id));
                }
                if o.seq.0 == 0 || o.seq.0 >= self.next_seq {
                    return Err(SnapshotError::SeqNotIssued(id));
                }
                if !ids.insert(id) {
                    return Err(SnapshotError::DuplicateId(id));
                }
                if let Some(p) = prev {
                    let price_ok = match side {
                        Side::Buy => o.price <= p.price,
                        Side::Sell => o.price >= p.price,
                    };
                    if !price_ok || (o.price == p.price && o.seq <= p.seq) {
                        return Err(SnapshotError::OutOfOrder(id));
                    }
                }
                prev = Some(o);
            }
        }
        if let (Some(bid), Some(ask)) = (self.bids.first(), self.asks.first())
            && bid.price >= ask.price
        {
            return Err(SnapshotError::Crossed);
        }
        Ok(())
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        if self.0.len() < n {
            return Err(SnapshotError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u64(&mut self) -> Result<u64, SnapshotError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().expect("took 8 bytes")))
    }

    fn i64(&mut self) -> Result<i64, SnapshotError> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes(b.try_into().expect("took 8 bytes")))
    }

    fn orders(&mut self, side: Side) -> Result<Vec<RestingOrder>, SnapshotError> {
        let n = self.u64()?;
        // Check the length before allocating so a huge count cannot exhaust memory.
        let needed = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_mul(ORDER_BYTES))
            .ok_or(SnapshotError::Truncated)?;
        if self.0.len() < needed {
            return Err(SnapshotError::Truncated);
        }
        let mut out = Vec::with_capacity(needed / ORDER_BYTES);
        for _ in 0..n {
            out.push(RestingOrder {
                id: OrderId(self.u64()?),
                side,
                price: Price(self.i64()?),
                qty: Qty(self.u64()?),
                seq: Seq(self.u64()?),
            });
        }
        Ok(out)
    }
}

/// 64-bit FNV-1a hash. Used for snapshot and event-stream digests.
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes
        .iter()
        .fold(OFFSET, |h, &b| (h ^ u64::from(b)).wrapping_mul(PRIME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: u64, side: Side, price: i64, qty: u64, seq: u64) -> RestingOrder {
        RestingOrder {
            id: OrderId(id),
            side,
            price: Price(price),
            qty: Qty(qty),
            seq: Seq(seq),
        }
    }

    fn sample() -> Snapshot {
        Snapshot {
            next_id: 10,
            next_seq: 12,
            bids: vec![
                order(3, Side::Buy, 100, 5, 3),
                order(7, Side::Buy, 100, 2, 9),
                order(1, Side::Buy, 99, 4, 1),
            ],
            asks: vec![
                order(2, Side::Sell, 101, 1, 11),
                order(4, Side::Sell, 105, 8, 4),
            ],
        }
    }

    #[test]
    fn fnv1a_matches_reference_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn encode_decode_round_trips() {
        let s = sample();
        let bytes = s.encode();
        assert_eq!(bytes.len(), 8 + 16 + 8 + 3 * 32 + 8 + 2 * 32);
        assert_eq!(Snapshot::decode(&bytes), Ok(s));
    }

    #[test]
    fn digest_depends_on_every_field() {
        let base = sample().digest();
        let mut s = sample();
        s.next_seq += 1;
        assert_ne!(s.digest(), base);
        let mut s = sample();
        s.bids[1].qty = Qty(3);
        assert_ne!(s.digest(), base);
        let mut s = sample();
        s.bids.swap(0, 1);
        assert_ne!(s.digest(), base);
    }

    #[test]
    fn malformed_bytes_are_rejected() {
        let bytes = sample().encode();
        assert_eq!(Snapshot::decode(b""), Err(SnapshotError::Truncated));
        assert_eq!(
            Snapshot::decode(b"LOBSNAP2xxxxxxxxxxxxxxxx"),
            Err(SnapshotError::BadMagic)
        );
        for cut in [9, 24, 40, bytes.len() - 1] {
            assert_eq!(
                Snapshot::decode(&bytes[..cut]),
                Err(SnapshotError::Truncated),
                "cut {cut}"
            );
        }
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(Snapshot::decode(&long), Err(SnapshotError::TrailingBytes));
    }

    #[test]
    fn huge_order_count_fails_without_allocating() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(Snapshot::decode(&bytes), Err(SnapshotError::Truncated));
    }

    #[test]
    fn invalid_states_are_rejected() {
        use SnapshotError as E;
        let check = |edit: fn(&mut Snapshot), expected| {
            let mut s = sample();
            edit(&mut s);
            assert_eq!(s.validate(), Err(expected));
            assert_eq!(Snapshot::decode(&s.encode()), Err(expected));
        };
        assert_eq!(sample().validate(), Ok(()));
        check(|s| s.next_id = 0, E::ZeroCounter);
        // Side is implied by section in the encoding, so this only arises for
        // snapshots built in memory.
        let mut s = sample();
        s.bids[0].side = Side::Sell;
        assert_eq!(s.validate(), Err(E::WrongSide(OrderId(3))));
        check(
            |s| s.asks[0].price = Price(0),
            E::PriceOutOfRange(OrderId(2)),
        );
        check(
            |s| s.asks[0].qty = Qty(0),
            E::QuantityOutOfRange(OrderId(2)),
        );
        check(|s| s.bids[0].id = OrderId(10), E::IdNotIssued(OrderId(10)));
        check(|s| s.bids[0].id = OrderId(0), E::IdNotIssued(OrderId(0)));
        check(|s| s.bids[0].seq = Seq(12), E::SeqNotIssued(OrderId(3)));
        check(|s| s.asks[1].id = OrderId(3), E::DuplicateId(OrderId(3)));
        check(|s| s.bids.swap(0, 1), E::OutOfOrder(OrderId(3)));
        check(|s| s.bids.swap(1, 2), E::OutOfOrder(OrderId(7)));
        check(|s| s.asks.swap(0, 1), E::OutOfOrder(OrderId(2)));
        check(|s| s.asks[0].price = Price(100), E::Crossed);
    }
}
