use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::feature::placed_features::PlacedFeature, world::WorldPortalExt};

/// Vanilla `SequenceFeature`: places its features in order and stops at the first that fails.
pub struct SequenceFeature {
    pub features: Vec<PlacedFeature>,
}

impl SequenceFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        self.features.iter().all(|feature| {
            feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            )
        })
    }
}
