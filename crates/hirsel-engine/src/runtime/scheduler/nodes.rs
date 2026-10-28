//! DICE 47675948 key_index.rs:35-76 separates interned keys from dense rows.
//! Like Incremental 98b5750 adjust_heights_heap.ml:137-202, an ordered edge
//! proves acyclicity without search. Hirsel instead repairs an exact finite
//! order interval by stable partition of exact reachable successors, retaining
//! failed and settled reads.
use super::*;
use std::ops::{Index, IndexMut};

/// Private to one locked runtime. No ID crosses its owner's boundary. Each
/// claim gets a fresh generation, including reuse of an evicted arena slot.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct NodeId {
    pub(super) slot: u32,
    generation: u32,
}
#[derive(Clone, Copy)]
pub(super) struct Edge {
    pub target: u32,
    token: u32,
}
#[derive(Clone, Copy)]
pub(super) struct Subscriber {
    waiter: NodeId,
    edge: u32,
}
#[derive(Default)]
pub(super) struct Nodes {
    index: BTreeMap<Key, u32>,
    edges: std::collections::HashMap<(u32, u32), u32>,
    keys: Vec<Option<Key>>,
    rows: Vec<Node>,
    free: Vec<u32>,
    order: Vec<u32>,
    serial: u32,
    repair_epoch: u32,
    pub repair_visits: u64,
}
impl Nodes {
    pub fn len(&self) -> usize {
        self.index.len()
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn has_slot(&self, key: &Key) -> bool {
        self.index.contains_key(key)
    }
    pub fn can_insert(&self, key: Key, limit: usize) -> bool {
        self.serial < u32::MAX
            && (self.index.contains_key(&key) || self.index.len() < limit.min(u32::MAX as usize))
    }
    pub fn get(&self, key: &Key) -> Option<&Node> {
        let node = &self.rows[*self.index.get(key)? as usize];
        (!matches!(node.status, Status::Dormant)).then_some(node)
    }
    pub fn get_mut(&mut self, key: &Key) -> Option<&mut Node> {
        let node = &mut self.rows[*self.index.get(key)? as usize];
        (!matches!(node.status, Status::Dormant)).then_some(node)
    }
    pub fn lookup(&self, key: Key) -> Option<NodeId> {
        let &slot = self.index.get(&key)?;
        Some(NodeId {
            slot,
            generation: self.rows[slot as usize].generation,
        })
    }
    pub fn node_id(&self, key: Key) -> Option<NodeId> {
        self.lookup(key).filter(|&id| self.get_id(id).is_some())
    }
    pub fn get_id(&self, id: NodeId) -> Option<&Node> {
        let node = self.rows.get(id.slot as usize)?;
        (node.generation == id.generation && !matches!(node.status, Status::Dormant))
            .then_some(node)
    }
    pub fn get_id_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let node = self.rows.get_mut(id.slot as usize)?;
        (node.generation == id.generation && !matches!(node.status, Status::Dormant))
            .then_some(node)
    }
    pub fn result(&self, id: NodeId) -> Option<Outcome> {
        match &self.get_id(id)?.status {
            Status::Settled(result) => Some(**result),
            _ => None,
        }
    }
    pub fn key_at(&self, slot: u32) -> Key {
        self.keys[slot as usize].expect("interned edge")
    }
    pub fn mark_live(&mut self, roots: impl Iterator<Item = Key>) {
        for node in &mut self.rows {
            node.live = false;
        }
        let mut pending: Vec<u32> = roots
            .filter_map(|key| self.index.get(&key).copied())
            .collect();
        while let Some(slot) = pending.pop() {
            let node = &mut self.rows[slot as usize];
            if !node.live && !matches!(node.status, Status::Dormant) {
                node.live = true;
                pending.extend(node.dependencies.iter().map(|edge| edge.target));
            }
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&Key, &Node)> {
        self.index.iter().filter_map(|(key, &slot)| {
            let node = &self.rows[slot as usize];
            (!matches!(node.status, Status::Dormant)).then_some((key, node))
        })
    }
    pub fn insert(&mut self, key: Key, mut node: Node) -> NodeId {
        self.serial = self.serial.checked_add(1).expect("admitted generation");
        node.generation = self.serial;
        let slot = if let Some(&slot) = self.index.get(&key) {
            let old = &mut self.rows[slot as usize];
            assert!(matches!(old.status, Status::Dormant));
            node.position = old.position;
            node.subscribers = std::mem::take(&mut old.subscribers);
            *old = node;
            slot
        } else {
            let slot = if let Some(slot) = self.free.pop() {
                node.position = self.rows[slot as usize].position;
                self.rows[slot as usize] = node;
                self.keys[slot as usize] = Some(key);
                slot
            } else {
                let slot = u32::try_from(self.rows.len()).expect("admitted slots");
                node.position = slot;
                self.rows.push(node);
                self.keys.push(Some(key));
                self.order.push(slot);
                slot
            };
            self.index.insert(key, slot);
            slot
        };
        NodeId {
            slot,
            generation: self.serial,
        }
    }
    fn reclaim(&mut self, slot: u32) {
        let node = &self.rows[slot as usize];
        if matches!(node.status, Status::Dormant)
            && node.subscribers.is_empty()
            && let Some(key) = self.keys[slot as usize].take()
        {
            self.index.remove(&key);
            self.free.push(slot);
        }
    }
    pub fn remove(&mut self, key: &Key) -> Option<Box<Execution>> {
        let &slot = self.index.get(key)?;
        let node = &mut self.rows[slot as usize];
        node.status = Status::Dormant;
        node.unsatisfied = 0;
        let generation = node.generation;
        let execution = node.execution.take();
        let dependencies = std::mem::take(&mut node.dependencies);
        for edge in dependencies {
            self.edges.remove(&(slot, edge.target));
            self.rows[edge.target as usize]
                .subscribers
                .retain(|s| s.waiter != NodeId { slot, generation });
            self.reclaim(edge.target);
        }
        self.reclaim(slot);
        execution
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
    #[cfg(test)]
    pub fn edge(&self, source: NodeId, target: Key) -> Option<usize> {
        let &slot = self.index.get(&target)?;
        self.edges
            .get(&(source.slot, slot))
            .map(|&edge| edge as usize)
    }
    pub fn edge_to(&self, source: NodeId, target: NodeId) -> Option<usize> {
        self.edges
            .get(&(source.slot, target.slot))
            .map(|&edge| edge as usize)
    }
    pub fn insert_edge(&mut self, source: NodeId, target: NodeId) -> Option<usize> {
        if source.slot == target.slot {
            return None;
        }
        let start = self.rows[target.slot as usize].position as usize;
        let end = self.rows[source.slot as usize].position as usize;
        if start <= end {
            // A path to source cannot leave this closed order interval. Move
            // only target's reachable successors after source, preserving both
            // partitions' old topological order. A marked-to-unmarked edge in
            // the interval is impossible by traversal closure.
            self.repair_epoch = self.repair_epoch.checked_add(1).unwrap_or_else(|| {
                for node in &mut self.rows {
                    node.visited = 0;
                }
                1
            });
            let epoch = self.repair_epoch;
            self.rows[target.slot as usize].visited = epoch;
            let mut pending = vec![target.slot];
            while let Some(slot) = pending.pop() {
                self.repair_visits += 1;
                for i in 0..self.rows[slot as usize].dependencies.len() {
                    self.repair_visits += 1;
                    let next = self.rows[slot as usize].dependencies[i].target;
                    if next == source.slot {
                        return None;
                    }
                    let node = &mut self.rows[next as usize];
                    if node.position as usize <= end && node.visited != epoch {
                        node.visited = epoch;
                        pending.push(next);
                    }
                }
            }
            // Reuse traversal capacity for the moved suffix. At most two
            // u32 vectors live here, bounded by twelve bytes per arena row.
            let mut ordered = Vec::with_capacity(end - start + 1);
            for &slot in &self.order[start..=end] {
                self.repair_visits += 1;
                if self.rows[slot as usize].visited == epoch {
                    pending.push(slot);
                } else {
                    ordered.push(slot);
                }
            }
            ordered.extend(pending);
            for (offset, &slot) in ordered.iter().enumerate() {
                self.repair_visits += 1;
                self.rows[slot as usize].position = (start + offset) as u32;
            }
            self.order[start..=end].copy_from_slice(&ordered);
        }
        let edge = self.rows[source.slot as usize].dependencies.len();
        self.edges.insert((source.slot, target.slot), edge as u32);
        self.rows[source.slot as usize].dependencies.push(Edge {
            target: target.slot,
            token: 0,
        });
        self.rows[target.slot as usize]
            .subscribers
            .push(Subscriber {
                waiter: source,
                edge: edge as u32,
            });
        Some(edge)
    }
    pub fn subscribe(&mut self, source: NodeId, edge: usize, producer: NodeId) -> bool {
        let node = &mut self.rows[source.slot as usize];
        assert_eq!(node.generation, source.generation);
        let token = &mut node.dependencies[edge].token;
        if *token == producer.generation {
            return false;
        }
        assert_eq!(*token, 0, "old subscription must drain before replacement");
        *token = producer.generation;
        node.unsatisfied += 1;
        true
    }
    pub fn subscribers(&self, producer: NodeId) -> &[Subscriber] {
        &self.rows[producer.slot as usize].subscribers
    }
    pub fn consume(&mut self, producer: NodeId, index: usize) -> Option<Key> {
        if self.rows[producer.slot as usize].generation != producer.generation {
            return None;
        }
        let subscription = *self.subscribers(producer).get(index)?;
        let waiter = &mut self.rows[subscription.waiter.slot as usize];
        if waiter.generation != subscription.waiter.generation {
            return None;
        }
        let edge = waiter.dependencies.get_mut(subscription.edge as usize)?;
        if edge.target != producer.slot || edge.token != producer.generation {
            return None;
        }
        edge.token = 0;
        waiter.unsatisfied = waiter
            .unsatisfied
            .checked_sub(1)
            .expect("one pending token");
        if waiter.unsatisfied == 0 && matches!(waiter.status, Status::Waiting) {
            waiter.status = Status::Ready;
            Some(self.key_at(subscription.waiter.slot))
        } else {
            None
        }
    }
    pub fn transfer_subscriptions(&mut self, key: Key) {
        let producer = self.node_id(key).expect("replacement");
        for i in 0..self.subscribers(producer).len() {
            let subscription = self.subscribers(producer)[i];
            let waiter = &mut self.rows[subscription.waiter.slot as usize];
            if waiter.generation == subscription.waiter.generation
                && let Some(edge) = waiter.dependencies.get_mut(subscription.edge as usize)
                && edge.target == producer.slot
                && edge.token != 0
            {
                edge.token = producer.generation;
            }
        }
    }
    pub fn allocated_bytes(&self) -> u64 {
        let capacity = self.edges.capacity();
        let buckets = if capacity == 0 {
            0
        } else {
            (capacity * 8).div_ceil(7).next_power_of_two().max(4)
        };
        // Include sparse B-tree nodes and hash control/bucket slack, as in
        // MemoGraph::retained_bytes. Vectors use allocated capacities; the
        // interval repair needs at most twelve extra bytes per arena row.
        let bytes = self.rows.capacity() * size_of::<Node>()
            + self.keys.capacity() * size_of::<Option<Key>>()
            + (self.free.capacity() + self.order.capacity()) * size_of::<u32>()
            + tree_bytes::<Key, u32>(self.index.len())
            + self.rows.len() * 12
            + buckets * (size_of::<((u32, u32), u32)>() + 1)
            + 32
            + self
                .rows
                .iter()
                .map(|n| {
                    n.dependencies.capacity() * size_of::<Edge>()
                        + n.subscribers.capacity() * size_of::<Subscriber>()
                        + n.execution
                            .as_ref()
                            .map_or(0, |_| size_of::<Execution>() + 16)
                        + if matches!(n.status, Status::Settled(_)) {
                            size_of::<Outcome>() + 16
                        } else {
                            0
                        }
                })
                .sum::<usize>();
        bytes as u64
    }
}
impl Index<&Key> for Nodes {
    type Output = Node;
    fn index(&self, key: &Key) -> &Node {
        self.get(key).expect("active query")
    }
}
impl IndexMut<&Key> for Nodes {
    fn index_mut(&mut self, key: &Key) -> &mut Node {
        self.get_mut(key).expect("active query")
    }
}
#[cfg(test)]
impl Nodes {
    pub fn rows_for_test(&self, id: NodeId) -> u32 {
        self.rows[id.slot as usize].position
    }
}

// Rust std BTree node.rs uses B=6, eleven entry slots and twelve child
// pointers. Non-root nodes have at least five entries. Include one sparse
// root even after the final removal; 64 bytes cover headers and alignment.
pub(super) fn tree_bytes<K, V>(len: usize) -> usize {
    (1 + len / 5) * (64 + 11 * (size_of::<K>() + size_of::<V>()) + 12 * size_of::<usize>())
}
