//! lock-freeの置換表。entryは16Bへpackし、XOR trickで破損を検出する。
//!
//! 各slotは2つの`AtomicU64`で、`guard = keymove ^ data`と`data`を持つ。
//! probeは`guard ^ data`でkeymoveを復元し、key検証子が合わなければ捨てる。
//! 2語が別々の書き込みから混ざった(torn write)場合、復元したkeymoveは
//! ほぼ確実に検証に落ちるので、lockなしでも壊れたentryを返さない。
//!
//! pack配置:
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

/// 16Bのslotを4つで64B、cache line 1本に収める。
const CLUSTER_SIZE: usize = 4;
const GENERATION_MASK: u32 = 0x3F;
/// 同じkeyの結果は、保存済みよりこの深さだけ浅くても新しい結果で置き換える。
/// 浅い再探索の結果はboundと最善手が新しく、少しの深さの差なら新しさを優先する。
const SAME_KEY_DEPTH_SLACK: u32 = 2;
/// 追い出す先を選ぶとき、1世代古いentryをこのdepth分だけ浅いものとみなす。
const AGE_DEPTH_WEIGHT: i32 = 2;
const MOVE_MASK: u64 = 0xFFFF_FFFF;
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

#[derive(Default)]
#[repr(align(64))]
struct Cluster([Slot; CLUSTER_SIZE]);

pub struct TranspositionTable {
    clusters: Box<[Cluster]>,
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
    let depth = stored_depth(data);
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

/// keyの検証子が一致するslotと、そこから読んだkeymove語・data語。
fn find(cluster: &[Slot], key: u64) -> Option<(&Slot, u64, u64)> {
    let verifier = key_verifier(key);
    cluster.iter().find_map(|slot| {
        let data = slot.data.load(Ordering::Relaxed);
        if data == 0 {
            return None;
        }
        let keymove = slot.guard.load(Ordering::Relaxed) ^ data;
        ((keymove >> 32) as u32 == verifier).then_some((slot, keymove, data))
    })
}

impl TranspositionTable {
    pub fn new(megabytes: usize) -> Self {
        let bytes = megabytes.max(1).saturating_mul(1024 * 1024);
        let cluster_count = (bytes / mem::size_of::<Cluster>()).max(1);
        let clusters = (0..cluster_count).map(|_| Cluster::default()).collect();
        Self { clusters, generation: AtomicU32::new(0) }
    }

    pub fn probe(&self, key: u64) -> Option<TtEntry> {
        find(self.cluster(key), key).map(|(_, keymove, data)| decode(key, keymove, data))
    }

    pub fn store(&self, entry: TtEntry) {
        let generation = self.generation.load(Ordering::Relaxed) & GENERATION_MASK;
        let (new_keymove, new_data) = encode(&entry, generation);
        let cluster = self.cluster(entry.key);

        // 同じkeyのslotがあれば、Exactか深さの差が小さい結果で置き換え、
        // 大きく浅ければ世代だけ更新する。新しい結果に手が無ければ既存の手を残す。
        if let Some((slot, keymove, data)) = find(cluster, entry.key) {
            if entry.bound == Bound::Exact
                || entry.depth + SAME_KEY_DEPTH_SLACK >= stored_depth(data)
            {
                let keymove = if entry.best_move.is_some() {
                    new_keymove
                } else {
                    (new_keymove & !MOVE_MASK) | (keymove & MOVE_MASK)
                };
                slot.write(keymove, new_data);
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

        // 満杯のclusterでは、世代の古さで割り引いたdepthが最も小さいentryを追い出す。
        // 新しい結果は必ず残す。捨てると同じ局面の再探索で手の情報が失われる。
        let victim = cluster
            .iter()
            .min_by_key(|slot| {
                let data = slot.data.load(Ordering::Relaxed);
                let age = generation.wrapping_sub(stored_generation(data)) & GENERATION_MASK;
                stored_depth(data) as i32 - AGE_DEPTH_WEIGHT * age as i32
            })
            .expect("a cluster is non-empty");
        victim.write(new_keymove, new_data);
    }

    pub fn new_search(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        for cluster in self.clusters.iter() {
            for slot in &cluster.0 {
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
        let (slot, _, data) =
            find(self.cluster(key), key).expect("corrupt_data_for_test: key not found");
        slot.data.store(data ^ (1 << SCORE_SHIFT), Ordering::Relaxed);
    }

    fn cluster(&self, key: u64) -> &[Slot; CLUSTER_SIZE] {
        &self.clusters[(key as usize) % self.clusters.len()].0
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
    fn a_cluster_is_one_cache_line() {
        assert_eq!(mem::size_of::<Cluster>(), 64);
        assert_eq!(mem::align_of::<Cluster>(), 64);
    }

    #[test]
    fn a_cluster_keeps_four_colliding_entries() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        for i in 0..4u32 {
            table.store(entry(5 + collision * u64::from(i), 6 - i, 60));
        }
        for i in 0..4u32 {
            let key = 5 + collision * u64::from(i);
            assert_eq!(table.probe(key), Some(entry(key, 6 - i, 60)));
        }
    }

    #[test]
    fn a_full_cluster_evicts_the_shallowest_entry_and_keeps_the_new_one() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        for (i, depth) in [6, 4, 8, 7].into_iter().enumerate() {
            table.store(entry(5 + collision * i as u64, depth, 0));
        }
        table.store(entry(5 + collision * 4, 1, 10));
        assert_eq!(table.probe(5 + collision), None, "最も浅いentryを追い出す");
        assert_eq!(table.probe(5), Some(entry(5, 6, 0)));
        assert_eq!(table.probe(5 + collision * 4), Some(entry(5 + collision * 4, 1, 10)));
    }

    #[test]
    fn an_old_generation_counts_as_shallower() {
        let table = TranspositionTable::new(1);
        let collision = table.cluster_count() as u64;
        table.store(entry(5, 6, 60));
        table.new_search();
        table.new_search();
        for i in 1..=4 {
            table.store(entry(5 + collision * i, 3, 0));
        }
        assert_eq!(table.probe(5), None, "2世代古いdepth 6はdepth 2相当として追い出す");
        assert_eq!(table.probe(5 + collision * 4), Some(entry(5 + collision * 4, 3, 0)));
    }

    #[test]
    fn same_key_does_not_replace_a_much_deeper_entry() {
        let table = TranspositionTable::new(1);
        let deep = TtEntry { bound: Bound::Lower, ..entry(9, 8, 80) };
        table.store(deep);
        table.store(TtEntry { bound: Bound::Upper, ..entry(9, 2, 21) });
        assert_eq!(table.probe(9), Some(deep));
    }

    #[test]
    fn same_key_keeps_the_stored_move_when_the_new_result_has_none() {
        let table = TranspositionTable::new(1);
        let mv = Move32::normal(
            rsshogi::types::Square::new(10),
            rsshogi::types::Square::new(20),
            rsshogi::types::Piece::B_SILVER,
        );
        table.store(TtEntry { best_move: Some(mv), ..entry(9, 4, 40) });
        table.store(entry(9, 5, 50));
        assert_eq!(table.probe(9), Some(TtEntry { best_move: Some(mv), ..entry(9, 5, 50) }));
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
