use std::mem;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};

use rsshogi::types::Move32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    Exact,
    Lower,
    Upper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtEntry {
    pub key: u64,
    pub depth: u32,
    pub score: i32,
    pub bound: Bound,
    pub best_move: Option<Move32>,
}

const CLUSTER_SIZE: usize = 2;

/// clusterのslotへ格納する形。置換の優先度判定のためにgenerationを添える。
#[derive(Debug, Clone, Copy)]
struct StoredEntry {
    entry: TtEntry,
    generation: u32,
}

pub struct TranspositionTable {
    clusters: Box<[RwLock<[Option<StoredEntry>; CLUSTER_SIZE]>]>,
    generation: AtomicU32,
}

impl TranspositionTable {
    pub fn new(megabytes: usize) -> Self {
        let bytes = megabytes.max(1).saturating_mul(1024 * 1024);
        let cluster_size = mem::size_of::<RwLock<[Option<StoredEntry>; CLUSTER_SIZE]>>().max(1);
        let cluster_count = (bytes / cluster_size).max(1);
        let clusters = (0..cluster_count).map(|_| RwLock::new([None; CLUSTER_SIZE])).collect();
        Self { clusters, generation: AtomicU32::new(0) }
    }

    pub fn probe(&self, key: u64) -> Option<TtEntry> {
        let cluster =
            self.clusters[self.index(key)].read().unwrap_or_else(|poisoned| poisoned.into_inner());
        cluster.iter().flatten().find(|stored| stored.entry.key == key).map(|stored| stored.entry)
    }

    pub fn store(&self, entry: TtEntry) {
        let generation = self.generation.load(Ordering::Relaxed);
        let mut cluster = self.clusters[self.index(entry.key)]
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(stored) =
            cluster.iter_mut().flatten().find(|stored| stored.entry.key == entry.key)
        {
            if entry.depth >= stored.entry.depth {
                *stored = StoredEntry { entry, generation };
            } else {
                stored.generation = generation;
            }
            return;
        }
        if let Some(empty) = cluster.iter_mut().find(|slot| slot.is_none()) {
            *empty = Some(StoredEntry { entry, generation });
            return;
        }

        let victim = cluster
            .iter()
            .enumerate()
            .min_by_key(|(_, stored)| {
                let stored = stored.expect("a full cluster has no empty entry");
                (stored.generation == generation, stored.entry.depth)
            })
            .map(|(index, _)| index)
            .expect("a cluster is non-empty");
        let old = cluster[victim].expect("victim must exist");
        if old.generation != generation || entry.depth >= old.entry.depth {
            cluster[victim] = Some(StoredEntry { entry, generation });
        }
    }

    pub fn new_search(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        for cluster in &self.clusters {
            *cluster.write().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                [None; CLUSTER_SIZE];
        }
    }

    #[cfg(test)]
    fn cluster_count(&self) -> usize {
        self.clusters.len()
    }

    fn index(&self, key: u64) -> usize {
        (key as usize) % self.clusters.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: u64, depth: u32, score: i32) -> TtEntry {
        TtEntry { key, depth, score, bound: Bound::Exact, best_move: None }
    }

    #[test]
    fn stores_probes_and_clears() {
        let table = TranspositionTable::new(1);
        table.store(entry(7, 3, 42));
        assert_eq!(table.probe(7), Some(entry(7, 3, 42)));
        table.clear();
        assert_eq!(table.probe(7), None);
    }

    #[test]
    fn a_cluster_keeps_two_colliding_entries() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        table.store(entry(5, 6, 60));
        table.store(entry(5 + collision, 4, 40));
        assert_eq!(table.probe(5), Some(entry(5, 6, 60)));
        assert_eq!(table.probe(5 + collision), Some(entry(5 + collision, 4, 40)));
    }

    #[test]
    fn a_shallow_current_generation_collision_keeps_deeper_entries() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        table.store(entry(5, 6, 60));
        table.store(entry(5 + collision, 4, 40));
        table.store(entry(5 + collision * 2, 2, 20));
        assert_eq!(table.probe(5), Some(entry(5, 6, 60)));
        assert_eq!(table.probe(5 + collision), Some(entry(5 + collision, 4, 40)));
        assert_eq!(table.probe(5 + collision * 2), None);
    }

    #[test]
    fn a_new_generation_can_replace_a_stale_entry() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        table.store(entry(5, 6, 60));
        table.store(entry(5 + collision, 4, 40));
        table.new_search();
        table.store(entry(5 + collision * 2, 1, 10));
        assert_eq!(table.probe(5), Some(entry(5, 6, 60)));
        assert_eq!(table.probe(5 + collision), None);
        assert_eq!(table.probe(5 + collision * 2), Some(entry(5 + collision * 2, 1, 10)));
    }

    #[test]
    fn same_key_does_not_replace_a_deeper_entry() {
        let table = TranspositionTable::new(1);
        table.store(entry(9, 8, 80));
        table.store(entry(9, 2, 21));
        assert_eq!(table.probe(9), Some(entry(9, 8, 80)));
    }
}
