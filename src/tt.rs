//! lock-freeの置換表。entryは16Bへpackし、XOR trickで破損を検出する。
//!
//! 各slotは2つの`AtomicU64`で、`guard = keymove ^ data`と`data`を持つ。
//! probeは`guard ^ data`でkeymoveを復元し、key検証子が合わなければ捨てる。
//! 2語が別々の書き込みから混ざった(torn write)場合、復元したkeymoveは
//! ほぼ確実に検証に落ちるので、lockなしでも壊れたentryを返さない。
//!
//! pack配置(decisionsはtask 0040参照):
//! - keymove語: [63:32] key検証子(高32bit^低32bitのfold)、[31:0] move32 raw
//! - data語: [63:48] score(i16)、[47:32] static_eval(i16、`i16::MIN`はNone)、
//!   [31:24] depth(u8)、[23:22] bound(1=Lower/2=Upper/3=Exact)、[21:16] 世代(6bit)
//!
//! boundに0を使わないため、有効なentryのdata語は0にならない。
//! `data == 0`は空slotを意味する。

use std::mem;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rsshogi::types::{MOVE_NONE, Move32};

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
    /// 手番側から見た静的評価。高価な再評価を省くための添え物で、
    /// 詰み系のentryなど評価していない局面では`None`。
    pub static_eval: Option<i32>,
}

const CLUSTER_SIZE: usize = 2;
const GENERATION_MASK: u32 = 0x3F;
/// static_evalの`None`を表す番兵。実評価は±`INF`(32,767)に収まり衝突しない。
const EVAL_NONE: i16 = i16::MIN;

const SCORE_SHIFT: u32 = 48;
const EVAL_SHIFT: u32 = 32;
const DEPTH_SHIFT: u32 = 24;
const BOUND_SHIFT: u32 = 22;
const GENERATION_SHIFT: u32 = 16;

/// XOR trickつきのslot。`guard = keymove ^ data`。
#[derive(Default)]
struct Slot {
    guard: AtomicU64,
    data: AtomicU64,
}

impl Slot {
    fn write(&self, keymove: u64, data: u64) {
        self.guard.store(keymove ^ data, Ordering::Relaxed);
        self.data.store(data, Ordering::Relaxed);
    }
}

pub struct TranspositionTable {
    clusters: Box<[[Slot; CLUSTER_SIZE]]>,
    generation: AtomicU32,
}

/// keyの検証子。cluster indexは剰余(下位側)から作るので、
/// 検証子は上下32bitのfoldでcluster内のkey違いを見分ける。
fn key_verifier(key: u64) -> u32 {
    (key ^ (key >> 32)) as u32
}

fn encode(entry: &TtEntry, generation: u32) -> (u64, u64) {
    debug_assert!(entry.depth <= u8::MAX as u32);
    debug_assert!(i16::try_from(entry.score).is_ok(), "TT scoreはi16に収まる");
    let score = entry.score.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    let eval = match entry.static_eval {
        Some(eval) if i16::try_from(eval).is_ok() && eval as i16 != EVAL_NONE => eval as i16,
        _ => EVAL_NONE,
    };
    let bound = match entry.bound {
        Bound::Lower => 1u64,
        Bound::Upper => 2,
        Bound::Exact => 3,
    };
    let keymove = (u64::from(key_verifier(entry.key)) << 32)
        | u64::from(entry.best_move.unwrap_or(MOVE_NONE).raw());
    let data = (u64::from(score as u16) << SCORE_SHIFT)
        | (u64::from(eval as u16) << EVAL_SHIFT)
        | (u64::from(entry.depth.min(u8::MAX as u32) as u8) << DEPTH_SHIFT)
        | (bound << BOUND_SHIFT)
        | (u64::from(generation & GENERATION_MASK) << GENERATION_SHIFT);
    (keymove, data)
}

fn decode(key: u64, keymove: u64, data: u64) -> TtEntry {
    let score = i32::from((data >> SCORE_SHIFT) as u16 as i16);
    let eval = (data >> EVAL_SHIFT) as u16 as i16;
    let depth = u32::from((data >> DEPTH_SHIFT) as u8);
    let bound = match (data >> BOUND_SHIFT) & 0b11 {
        1 => Bound::Lower,
        2 => Bound::Upper,
        _ => Bound::Exact,
    };
    let move_raw = keymove as u32;
    TtEntry {
        key,
        depth,
        score,
        bound,
        best_move: (move_raw != 0).then(|| Move32::from_raw(move_raw)),
        static_eval: (eval != EVAL_NONE).then_some(i32::from(eval)),
    }
}

fn stored_generation(data: u64) -> u32 {
    ((data >> GENERATION_SHIFT) as u32) & GENERATION_MASK
}

fn stored_depth(data: u64) -> u32 {
    u32::from((data >> DEPTH_SHIFT) as u8)
}

impl TranspositionTable {
    pub fn new(megabytes: usize) -> Self {
        let bytes = megabytes.max(1).saturating_mul(1024 * 1024);
        let cluster_size = mem::size_of::<[Slot; CLUSTER_SIZE]>().max(1);
        let cluster_count = (bytes / cluster_size).max(1);
        let clusters = (0..cluster_count).map(|_| <[Slot; CLUSTER_SIZE]>::default()).collect();
        Self { clusters, generation: AtomicU32::new(0) }
    }

    pub fn probe(&self, key: u64) -> Option<TtEntry> {
        let verifier = key_verifier(key);
        for slot in &self.clusters[self.index(key)] {
            let data = slot.data.load(Ordering::Relaxed);
            if data == 0 {
                continue;
            }
            let keymove = slot.guard.load(Ordering::Relaxed) ^ data;
            if (keymove >> 32) as u32 != verifier {
                continue;
            }
            return Some(decode(key, keymove, data));
        }
        None
    }

    pub fn store(&self, entry: TtEntry) {
        let generation = self.generation.load(Ordering::Relaxed) & GENERATION_MASK;
        let verifier = key_verifier(entry.key);
        let (new_keymove, new_data) = encode(&entry, generation);
        let cluster = &self.clusters[self.index(entry.key)];

        // 同じkeyのslotがあれば、深い結果だけを置き換え、浅ければ世代のみ更新する。
        for slot in cluster {
            let data = slot.data.load(Ordering::Relaxed);
            if data == 0 {
                continue;
            }
            let keymove = slot.guard.load(Ordering::Relaxed) ^ data;
            if (keymove >> 32) as u32 != verifier {
                continue;
            }
            if entry.depth >= stored_depth(data) {
                slot.write(new_keymove, new_data);
            } else {
                let refreshed = (data & !(u64::from(GENERATION_MASK) << GENERATION_SHIFT))
                    | (u64::from(generation) << GENERATION_SHIFT);
                slot.write(keymove, refreshed);
            }
            return;
        }
        if let Some(empty) = cluster.iter().find(|slot| slot.data.load(Ordering::Relaxed) == 0) {
            empty.write(new_keymove, new_data);
            return;
        }

        // 満杯のclusterでは、古い世代・浅いentryから追い出す。
        let victim = cluster
            .iter()
            .min_by_key(|slot| {
                let data = slot.data.load(Ordering::Relaxed);
                (stored_generation(data) == generation, stored_depth(data))
            })
            .expect("a cluster is non-empty");
        let old_data = victim.data.load(Ordering::Relaxed);
        if stored_generation(old_data) != generation || entry.depth >= stored_depth(old_data) {
            victim.write(new_keymove, new_data);
        }
    }

    pub fn new_search(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        for cluster in self.clusters.iter() {
            for slot in cluster {
                slot.write(0, 0);
            }
        }
    }

    #[cfg(test)]
    fn cluster_count(&self) -> usize {
        self.clusters.len()
    }

    /// XOR trickのテスト用に、keyの載っているslotのdata語を1bit壊す。
    #[cfg(test)]
    fn corrupt_data_for_test(&self, key: u64) {
        let verifier = key_verifier(key);
        for slot in &self.clusters[self.index(key)] {
            let data = slot.data.load(Ordering::Relaxed);
            if data == 0 {
                continue;
            }
            let keymove = slot.guard.load(Ordering::Relaxed) ^ data;
            if (keymove >> 32) as u32 == verifier {
                slot.data.store(data ^ (1 << SCORE_SHIFT), Ordering::Relaxed);
                return;
            }
        }
        panic!("corrupt_data_for_test: key not found");
    }

    fn index(&self, key: u64) -> usize {
        (key as usize) % self.clusters.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: u64, depth: u32, score: i32) -> TtEntry {
        TtEntry { key, depth, score, bound: Bound::Exact, best_move: None, static_eval: None }
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
    fn every_field_round_trips_through_the_packed_entry() {
        let table = TranspositionTable::new(1);
        let mv = Move32::normal(
            rsshogi::types::Square::new(10),
            rsshogi::types::Square::new(20),
            rsshogi::types::Piece::B_SILVER,
        );
        for (bound, score, eval) in [
            (Bound::Lower, 31_999, Some(-321)),
            (Bound::Upper, -31_999, Some(0)),
            (Bound::Exact, -1, None),
        ] {
            let stored = TtEntry {
                key: 0xDEAD_BEEF_1234_5678,
                depth: 64,
                score,
                bound,
                best_move: Some(mv),
                static_eval: eval,
            };
            table.store(stored);
            assert_eq!(table.probe(stored.key), Some(stored));
        }
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

    #[test]
    fn a_different_key_in_the_same_cluster_does_not_read_a_foreign_entry() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        table.store(entry(5, 6, 60));
        assert_eq!(table.probe(5 + collision), None, "同じclusterでもkey検証子で弾く");
    }

    #[test]
    fn a_corrupted_slot_is_never_returned() {
        let table = TranspositionTable::new(1);
        table.store(entry(7, 3, 42));
        table.corrupt_data_for_test(7);
        assert_eq!(table.probe(7), None, "guardとdataの不整合はXOR trickで検出する");
    }
}
