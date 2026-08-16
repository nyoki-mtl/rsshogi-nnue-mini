//! accumulatorと、その差分更新を記録する型。

use super::TRANSFORMED_DIMENSIONS;

/// 一手が動かせるfeatureは、移動元・捕獲駒の除去と、移動先・持ち駒の追加が上限。
pub(super) const MAX_FEATURE_CHANGES: usize = 2;

#[derive(Clone)]
pub struct StandardAccumulator {
    pub(super) accumulations: [[i16; TRANSFORMED_DIMENSIONS]; 2],
    pub(super) feature_zero: [usize; 2],
    pub(super) history: Vec<AccumulatorDelta>,
}

#[derive(Clone)]
pub(super) struct AccumulatorDelta {
    pub(super) previous_feature_zero: [usize; 2],
    pub(super) perspectives: [PerspectiveDelta; 2],
}

#[derive(Clone)]
pub(super) enum PerspectiveDelta {
    Changed(FeatureChanges),
    Refreshed(Box<[i16; TRANSFORMED_DIMENSIONS]>),
}

#[derive(Clone, Copy, Default)]
pub(super) struct FeatureChanges {
    removed: [usize; MAX_FEATURE_CHANGES],
    removed_length: usize,
    added: [usize; MAX_FEATURE_CHANGES],
    added_length: usize,
}

impl FeatureChanges {
    pub(super) fn remove(&mut self, feature: usize) {
        debug_assert!(
            self.removed_length < MAX_FEATURE_CHANGES,
            "a move removes at most two features"
        );
        self.removed[self.removed_length] = feature;
        self.removed_length += 1;
    }

    pub(super) fn add(&mut self, feature: usize) {
        debug_assert!(self.added_length < MAX_FEATURE_CHANGES, "a move adds at most two features");
        self.added[self.added_length] = feature;
        self.added_length += 1;
    }

    pub(super) fn removed(&self) -> &[usize] {
        &self.removed[..self.removed_length]
    }

    pub(super) fn added(&self) -> &[usize] {
        &self.added[..self.added_length]
    }

    pub(super) const fn reversed(self) -> Self {
        Self {
            removed: self.added,
            removed_length: self.added_length,
            added: self.removed,
            added_length: self.removed_length,
        }
    }
}
