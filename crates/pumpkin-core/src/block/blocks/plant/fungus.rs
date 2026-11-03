use crate::block::GetStateForNeighborUpdateArgs;
use crate::block::{BlockBehaviour, BlockMetadata, BonemealArgs, CanPlaceAtArgs};
use crate::world::generation_cache::WorldGenerationCache;
use pumpkin_data::BlockStateId;
use pumpkin_data::configured_feature::ConfiguredFeature as FeatureKey;
use pumpkin_data::placed_feature::PlacedFeature;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockId, tag};
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::generation::feature::configured_features::CONFIGURED_FEATURES;
use pumpkin_world::generation::height_limit::HeightLimitView;

/// Vanilla `NetherFungusBlock.BONEMEAL_SUCCESS_PROBABILITY`.
const BONEMEAL_SUCCESS_PROBABILITY: f32 = 0.4;

pub struct FungusBlock;

impl BlockMetadata for FungusBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::CRIMSON_FUNGUS, BlockId::WARPED_FUNGUS].into()
    }
}

impl BlockBehaviour for FungusBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let block_below = args.block_accessor.get_block(&args.position.down());
        if args.block == &Block::WARPED_FUNGUS {
            block_below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_WARPED_FUNGUS)
        } else {
            block_below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_CRIMSON_FUNGUS)
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let block_below = args.world.get_block(&args.position.down());
        let can_survive = if args.block == &Block::WARPED_FUNGUS {
            block_below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_WARPED_FUNGUS)
        } else {
            block_below.has_tag(&tag::Block::MINECRAFT_SUPPORTS_CRIMSON_FUNGUS)
        };
        if !can_survive {
            return Block::AIR.default_state.id;
        }
        args.state_id
    }

    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        let required = if args.block == &Block::WARPED_FUNGUS {
            &Block::WARPED_NYLIUM
        } else {
            &Block::CRIMSON_NYLIUM
        };
        args.world.get_block(&args.position.down()) == required
            && args.world.is_in_height_limit(args.position.0.y + 1)
    }

    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        game_random().random::<f32>() < BONEMEAL_SUCCESS_PROBABILITY
    }

    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        // The placed feature only matters to biome filters, which the planted fungi lack.
        let (key, placed) = if args.block == &Block::WARPED_FUNGUS {
            (FeatureKey::WarpedFungusPlanted, PlacedFeature::WarpedFungi)
        } else {
            (
                FeatureKey::CrimsonFungusPlanted,
                PlacedFeature::CrimsonFungi,
            )
        };
        let Some(feature) = CONFIGURED_FEATURES.get(&key) else {
            return;
        };
        let portal = args.world.level.world_portal.load_full();
        let Some(portal) = portal.as_ref() else {
            return;
        };
        let mut cache = WorldGenerationCache::new(args.world.clone(), args.position);
        let mut random = RandomGenerator::Gameplay(game_random());
        let (min_y, height) = (cache.bottom_y(), cache.height());
        feature.generate(
            &mut cache,
            &**portal,
            min_y,
            height,
            placed,
            &mut random,
            *args.position,
        );
        cache.apply();
    }
}
