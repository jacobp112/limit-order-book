//! Order storage and per-price FIFO queues.
//!
//! Every resting order lives in one [`Arena`] slot. A [`Level`] threads its
//! orders into a doubly linked list through those slots, which gives O(1)
//! append at the tail, removal at the head and removal from the middle.

use crate::types::{OrderId, Price, Qty, Seq, Side};

/// Index of an arena slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Handle(u32);

/// A resting order and its queue links.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) id: OrderId,
    pub(crate) side: Side,
    pub(crate) price: Price,
    pub(crate) open: Qty,
    pub(crate) seq: Seq,
    prev: Option<Handle>,
    next: Option<Handle>,
}

/// Slot storage for nodes, reusing freed slots.
///
/// A slot is `None` while free, so a stale handle panics on access instead of
/// silently reading another order.
#[derive(Debug, Default)]
pub(crate) struct Arena {
    slots: Vec<Option<Node>>,
    free: Vec<u32>,
}

impl Arena {
    pub(crate) fn alloc(
        &mut self,
        id: OrderId,
        side: Side,
        price: Price,
        open: Qty,
        seq: Seq,
    ) -> Handle {
        let node = Node {
            id,
            side,
            price,
            open,
            seq,
            prev: None,
            next: None,
        };
        if let Some(slot) = self.free.pop() {
            self.slots[slot as usize] = Some(node);
            Handle(slot)
        } else {
            let slot = u32::try_from(self.slots.len()).expect("more than u32::MAX resting orders");
            self.slots.push(Some(node));
            Handle(slot)
        }
    }

    pub(crate) fn free(&mut self, h: Handle) -> Node {
        let node = self.slots[h.0 as usize]
            .take()
            .expect("double free of arena slot");
        self.free.push(h.0);
        node
    }

    pub(crate) fn get(&self, h: Handle) -> &Node {
        self.slots[h.0 as usize].as_ref().expect("stale handle")
    }

    pub(crate) fn get_mut(&mut self, h: Handle) -> &mut Node {
        self.slots[h.0 as usize].as_mut().expect("stale handle")
    }
}

/// All resting orders at one price on one side, in time priority.
#[derive(Debug, Default)]
pub(crate) struct Level {
    head: Option<Handle>,
    tail: Option<Handle>,
    total: Qty,
    count: u32,
}

impl Level {
    pub(crate) fn head(&self) -> Option<Handle> {
        self.head
    }

    pub(crate) fn total(&self) -> Qty {
        self.total
    }

    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    /// Appends `h` at the back of the queue.
    pub(crate) fn push_back(&mut self, arena: &mut Arena, h: Handle) {
        let open = {
            let node = arena.get_mut(h);
            debug_assert!(node.prev.is_none() && node.next.is_none());
            node.prev = self.tail;
            node.open
        };
        match self.tail {
            Some(t) => arena.get_mut(t).next = Some(h),
            None => self.head = Some(h),
        }
        self.tail = Some(h);
        self.total = self
            .total
            .checked_add(open)
            .expect("level total bounded by MAX_QTY * u32::MAX");
        self.count = self.count.checked_add(1).expect("arena caps order count");
    }

    /// Removes `h` from anywhere in the queue. The slot stays allocated.
    pub(crate) fn unlink(&mut self, arena: &mut Arena, h: Handle) {
        let (prev, next, open) = {
            let node = arena.get_mut(h);
            (node.prev.take(), node.next.take(), node.open)
        };
        match prev {
            Some(p) => arena.get_mut(p).next = next,
            None => self.head = next,
        }
        match next {
            Some(n) => arena.get_mut(n).prev = prev,
            None => self.tail = prev,
        }
        self.total = self
            .total
            .checked_sub(open)
            .expect("level total covers its orders");
        self.count = self
            .count
            .checked_sub(1)
            .expect("level count covers its orders");
    }

    /// Reduces the open quantity of `h` in place, keeping its position.
    pub(crate) fn reduce(&mut self, arena: &mut Arena, h: Handle, by: Qty) {
        let node = arena.get_mut(h);
        node.open = node.open.checked_sub(by).expect("reduce within open qty");
        self.total = self
            .total
            .checked_sub(by)
            .expect("level total covers its orders");
    }

    /// Walks the queue checking that `prev` links mirror `next` links and
    /// that the walk ends at `tail`, visiting at most `max` nodes so a
    /// corrupted (cyclic) list cannot loop forever. Returns the handles.
    pub(crate) fn walk_checked(
        &self,
        arena: &Arena,
        max: usize,
    ) -> Result<Vec<Handle>, &'static str> {
        let mut out = Vec::new();
        let mut prev = None;
        let mut cur = self.head;
        while let Some(h) = cur {
            if out.len() >= max {
                return Err("queue longer than the number of indexed orders (cycle?)");
            }
            let node = arena.get(h);
            if node.prev != prev {
                return Err("prev link does not match traversal order");
            }
            out.push(h);
            prev = Some(h);
            cur = node.next;
        }
        if prev != self.tail {
            return Err("tail does not match last node");
        }
        Ok(out)
    }

    #[cfg(test)]
    pub(crate) fn corrupt_total(&mut self, total: Qty) {
        self.total = total;
    }

    #[cfg(test)]
    pub(crate) fn corrupt_tail(&mut self, tail: Option<Handle>) {
        self.tail = tail;
    }

    /// Iterates handles from head (oldest) to tail.
    pub(crate) fn iter<'a>(&self, arena: &'a Arena) -> LevelIter<'a> {
        LevelIter {
            arena,
            next: self.head,
        }
    }
}

/// Head-to-tail iterator over a level's handles.
#[derive(Debug)]
pub(crate) struct LevelIter<'a> {
    arena: &'a Arena,
    next: Option<Handle>,
}

impl Iterator for LevelIter<'_> {
    type Item = Handle;

    fn next(&mut self) -> Option<Handle> {
        let h = self.next?;
        self.next = self.arena.get(h).next;
        Some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(qtys: &[u64]) -> (Arena, Level, Vec<Handle>) {
        let mut arena = Arena::default();
        let mut level = Level::default();
        let mut handles = Vec::new();
        for (i, &q) in (1u64..).zip(qtys) {
            let h = arena.alloc(OrderId(i), Side::Buy, Price(100), Qty(q), Seq(i));
            level.push_back(&mut arena, h);
            handles.push(h);
        }
        (arena, level, handles)
    }

    fn ids(level: &Level, arena: &Arena) -> Vec<u64> {
        level.iter(arena).map(|h| arena.get(h).id.0).collect()
    }

    #[test]
    fn push_back_preserves_arrival_order() {
        let (arena, level, _) = setup(&[5, 7, 9]);
        assert_eq!(ids(&level, &arena), [1, 2, 3]);
        assert_eq!(level.total(), Qty(21));
        assert_eq!(level.count(), 3);
    }

    #[test]
    fn unlink_head_middle_and_tail() {
        let (mut arena, mut level, h) = setup(&[1, 2, 3, 4]);
        level.unlink(&mut arena, h[1]);
        assert_eq!(ids(&level, &arena), [1, 3, 4]);
        level.unlink(&mut arena, h[0]);
        assert_eq!(ids(&level, &arena), [3, 4]);
        level.unlink(&mut arena, h[3]);
        assert_eq!(ids(&level, &arena), [3]);
        assert_eq!((level.total(), level.count()), (Qty(3), 1));
        level.unlink(&mut arena, h[2]);
        assert!(level.is_empty());
        assert_eq!((level.total(), level.count()), (Qty::ZERO, 0));
    }

    #[test]
    fn unlinked_order_can_rejoin_at_back() {
        let (mut arena, mut level, h) = setup(&[1, 2, 3]);
        level.unlink(&mut arena, h[0]);
        level.push_back(&mut arena, h[0]);
        assert_eq!(ids(&level, &arena), [2, 3, 1]);
        assert_eq!(level.total(), Qty(6));
    }

    #[test]
    fn reduce_keeps_position_and_updates_total() {
        let (mut arena, mut level, h) = setup(&[10, 20]);
        level.reduce(&mut arena, h[0], Qty(4));
        assert_eq!(ids(&level, &arena), [1, 2]);
        assert_eq!(arena.get(h[0]).open, Qty(6));
        assert_eq!(level.total(), Qty(26));
    }

    #[test]
    fn freed_slots_are_reused() {
        let (mut arena, mut level, h) = setup(&[1, 2]);
        level.unlink(&mut arena, h[0]);
        arena.free(h[0]);
        let again = arena.alloc(OrderId(9), Side::Sell, Price(1), Qty(1), Seq(9));
        assert_eq!(again, h[0]);
        assert_eq!(arena.get(again).id, OrderId(9));
    }

    #[test]
    #[should_panic(expected = "stale handle")]
    fn stale_handle_panics() {
        let (mut arena, mut level, h) = setup(&[1]);
        level.unlink(&mut arena, h[0]);
        arena.free(h[0]);
        let _ = arena.get(h[0]);
    }
}
