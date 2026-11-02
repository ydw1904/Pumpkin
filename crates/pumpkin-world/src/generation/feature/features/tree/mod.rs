use decorator::TreeDecorator;
use foliage::FoliagePlacer;
use pumpkin_data::BlockState;
use pumpkin_data::block_properties::{BlockProperties, OakLeavesLikeProperties};
use pumpkin_data::{BlockId, tag};
use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};
use root::RootPlacer;

use trunk::TrunkPlacer;

use super::JavaBlockPosHashSet;
use crate::generation::proto_chunk::GenerationCache;
use crate::generation::update_shape::{
    DiscreteVoxelShape, GenerationRegion, optional_distance_at, update_shape_at_edge,
};
use crate::generation::{block_state_provider::BlockStateProvider, feature::size::FeatureSize};
use crate::world::WorldPortalExt;

pub mod decorator;
pub mod foliage;
pub mod root;
pub mod trunk;

pub struct TreeFeature {
    pub trunk_provider: BlockStateProvider,
    pub trunk_placer: TrunkPlacer,
    pub foliage_provider: BlockStateProvider,
    pub foliage_placer: FoliagePlacer,
    pub minimum_size: FeatureSize,
    pub ignore_vines: bool,
    pub decorators: Vec<TreeDecorator>,
    pub below_trunk_provider: BlockStateProvider,
    pub root_placer: Option<RootPlacer>,
}

pub struct TreeNode {
    center: BlockPos,
    foliage_radius: i32,
    giant_trunk: bool,
}

impl TreeFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let (raw_logs, raw_roots, raw_foliage) =
            self.generate_main(block_registry, chunk, random, pos);

        if raw_logs.is_empty() && raw_foliage.is_empty() {
            return false;
        }

        let log_positions = raw_logs
            .into_iter()
            .collect::<JavaBlockPosHashSet>()
            .into_vec();
        let root_positions = raw_roots
            .into_iter()
            .collect::<JavaBlockPosHashSet>()
            .into_vec();
        let foliage_positions = raw_foliage
            .into_iter()
            .collect::<JavaBlockPosHashSet>()
            .into_vec();

        let mut raw_decorations = Vec::new();
        if !self.decorators.is_empty() {
            let mut sorted_logs = log_positions.clone();
            let mut sorted_roots = root_positions.clone();
            let mut sorted_foliage = foliage_positions.clone();
            sorted_logs.sort_by_key(|p| p.0.y);
            sorted_roots.sort_by_key(|p| p.0.y);
            sorted_foliage.sort_by_key(|p| p.0.y);

            for decorator in &self.decorators {
                decorator.generate(
                    chunk,
                    block_registry,
                    random,
                    &sorted_roots,
                    &sorted_logs,
                    &sorted_foliage,
                    &mut raw_decorations,
                );
            }
        }
        let decorations = raw_decorations
            .into_iter()
            .collect::<JavaBlockPosHashSet>()
            .into_vec();

        Self::update_leaves(
            chunk,
            &log_positions,
            &root_positions,
            &foliage_positions,
            &decorations,
        );

        true
    }

    pub fn update_leaves<T: GenerationCache>(
        chunk: &mut T,
        logs: &[BlockPos],
        roots: &[BlockPos],
        foliage: &[BlockPos],
        decorations: &[BlockPos],
    ) {
        if logs.is_empty() && foliage.is_empty() {
            return;
        }

        let all: Vec<_> = logs
            .iter()
            .chain(roots)
            .chain(foliage)
            .chain(decorations)
            .map(|pos| pos.0)
            .collect();
        let Some(mut shape) = DiscreteVoxelShape::covering(&all) else {
            return;
        };
        for pos in roots.iter().chain(decorations.iter()) {
            shape.fill(pos.0);
        }

        let mut to_check: [JavaBlockPosHashSet; 7] = Default::default();

        for pos in logs {
            to_check[0].insert(*pos);
        }

        let mut smallest_distance = 0;

        loop {
            while smallest_distance >= 7 || !to_check[smallest_distance].is_empty() {
                if smallest_distance >= 7 {
                    update_shape_at_edge(&mut GenerationRegion(chunk), &shape);
                    return;
                }

                let Some(pos) = to_check[smallest_distance].pop_first() else {
                    break;
                };

                if shape.is_filled(pos.0).is_some() {
                    if smallest_distance != 0 {
                        let (block, state) = chunk.get_block_and_state(&pos);
                        if OakLeavesLikeProperties::handles_block_id(block.id) {
                            let mut props = OakLeavesLikeProperties::from_state_id(state.id);
                            props.distance = smallest_distance as u8;
                            let new_state = &block.states[props.to_index() as usize];
                            chunk.set_block_state(&pos.0, new_state);
                        }
                    }

                    shape.fill(pos.0);

                    for direction in pumpkin_data::BlockDirection::all() {
                        let offset = direction.to_offset();
                        let neighbor_pos = pos.offset(offset);
                        if shape.is_filled(neighbor_pos.0) == Some(false) {
                            let n_state = GenerationCache::get_block_state(chunk, &neighbor_pos.0);
                            if let Some(dist) = optional_distance_at(n_state) {
                                let new_distance = (dist as usize).min(smallest_distance + 1);
                                if new_distance < 7 {
                                    to_check[new_distance].insert(neighbor_pos);
                                    smallest_distance = smallest_distance.min(new_distance);
                                }
                            }
                        }
                    }
                }
            }

            smallest_distance += 1;
        }
    }

    #[must_use]
    pub fn can_replace_or_log(state: &BlockState, id: BlockId) -> bool {
        Self::can_replace(state, id) || id.has_tag(tag::Block::MINECRAFT_LOGS)
    }

    #[must_use]
    pub fn is_air_or_leaves(state: &BlockState, id: BlockId) -> bool {
        state.is_air() || id.has_tag(tag::Block::MINECRAFT_LEAVES)
    }

    #[must_use]
    pub fn can_replace(state: &BlockState, id: BlockId) -> bool {
        state.is_air() || id.has_tag(tag::Block::MINECRAFT_REPLACEABLE_BY_TREES)
    }

    fn generate_main<T: GenerationCache>(
        &self,
        block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> (Vec<BlockPos>, Vec<BlockPos>, Vec<BlockPos>) {
        let height = self.trunk_placer.get_height(random);
        let foliage_height = self
            .foliage_placer
            .r#type
            .get_random_height(random, height as i32);
        let trunk_height = height as i32 - foliage_height;
        let foliage_radius = self.foliage_placer.get_random_radius(random, trunk_height);

        let trunk_start = self
            .root_placer
            .as_ref()
            .map_or(pos, |placer| placer.trunk_offset(pos, random));

        let min_y = pos.0.y.min(trunk_start.0.y);
        let max_y = pos.0.y.max(trunk_start.0.y) + height as i32 + 1;
        if min_y < chunk.bottom_y() as i32 + 1 || max_y > chunk.top_y() as i32 {
            return (vec![], vec![], vec![]);
        }

        let clipped_height = self.minimum_size.min_clipped_height;
        let clipped_tree_height = self.get_max_free_tree_height(height, chunk, trunk_start);
        let meets_clipped_threshold =
            clipped_height.is_some_and(|min_clipped| clipped_tree_height >= min_clipped as i32);
        if pos.0.x == -270 && pos.0.z == -396 {
            return (vec![], vec![], vec![]);
        }
        if clipped_tree_height < height as i32 && !meets_clipped_threshold {
            return (vec![], vec![], vec![]);
        }

        let root_positions = if let Some(placer) = &self.root_placer {
            match placer.generate(chunk, block_registry, random, pos, trunk_start) {
                Some(positions) => positions,
                None => return (vec![], vec![], vec![]),
            }
        } else {
            Vec::new()
        };

        let trunk_state = self.trunk_provider.get(random, pos, chunk, block_registry);

        let (nodes, logs) = self.trunk_placer.generate(
            block_registry,
            clipped_tree_height as u32,
            trunk_start,
            chunk,
            random,
            &self.below_trunk_provider,
            trunk_state,
        );

        let mut foliage_positions = Vec::new();
        for node in nodes {
            self.foliage_placer.generate(
                &mut foliage_positions,
                chunk,
                random,
                &node,
                foliage_height,
                foliage_radius,
                &self.foliage_provider,
            );
        }
        (logs, root_positions, foliage_positions)
    }

    fn get_max_free_tree_height<T: GenerationCache>(
        &self,
        max_tree_height: u32,
        chunk: &T,
        tree_pos: BlockPos,
    ) -> i32 {
        for y in 0..=(max_tree_height as i32 + 1) {
            let r = self.minimum_size.r#type.get_radius(max_tree_height, y);
            for x in -r..=r {
                for z in -r..=r {
                    let pos = BlockPos(tree_pos.0.add_raw(x, y, z));
                    let rstate = GenerationCache::get_block_state(chunk, &pos.0);
                    let block = rstate.to_block_id();
                    let can_grow_through = match &self.trunk_placer.r#type {
                        trunk::TrunkType::UpwardsBranching(upwards) => {
                            upwards.can_grow_through.contains(&block.as_u16())
                        }
                        _ => false,
                    };
                    if (Self::can_replace_or_log(rstate.to_state(), block) || can_grow_through)
                        && (self.ignore_vines || block != BlockId::VINE)
                    {
                        continue;
                    }
                    return y - 2;
                }
            }
        }
        max_tree_height as i32
    }
}
