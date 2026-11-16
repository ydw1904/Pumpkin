use std::sync::Arc;

use pumpkin_data::{
    BlockDirection, BlockStateId,
    block_properties::CandleLikeProperties,
    entity::EntityPose,
    fluid::Fluid,
    game_event::GameEvent,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockAccessor;
use pumpkin_world::world::BlockFlags;

use crate::{
    block::{
        BlockBehaviour, BlockIsReplacing, CanPlaceAtArgs, CanUpdateAtArgs, ExplodeArgs,
        GetStateForNeighborUpdateArgs, NormalUseArgs, OnPlaceArgs, OnProjectileHitArgs,
        can_support_center, registry::BlockActionResult,
    },
    entity::EntityBase,
    world::World,
};

#[pumpkin_block_from_tag("minecraft:candles")]
pub struct CandleBlock;

impl BlockBehaviour for CandleBlock {
    fn on_explosion_hit(&self, args: ExplodeArgs<'_>) {
        let mut properties =
            CandleLikeProperties::from_state_id(args.world.get_block_state_id(args.position));
        if properties.lit {
            properties.lit = false;
            extinguish(
                args.world,
                args.position,
                properties.to_state_id(args.block),
                None,
            );
        }
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if let BlockIsReplacing::Itself(state_id) = args.replacing {
            let mut properties = CandleLikeProperties::from_state_id(state_id);
            if properties.candles < 4 {
                properties.candles += 1;
            }
            return properties.to_state_id(args.block);
        }

        let mut properties = CandleLikeProperties::default(args.block);
        properties.waterlogged = args.replacing.water_source();
        properties.to_state_id(args.block)
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let state_id = args.world.get_block_state_id(args.position);
        let mut properties = CandleLikeProperties::from_state_id(state_id);

        if args.player.may_build() && properties.lit {
            properties.lit = false;
            extinguish(
                args.world,
                args.position,
                properties.to_state_id(args.block),
                Some(args.player.as_ref()),
            );
            return BlockActionResult::Success;
        }

        BlockActionResult::Pass
    }

    fn on_projectile_hit(&self, args: OnProjectileHitArgs<'_>) {
        let mut properties = CandleLikeProperties::from_state_id(args.state.id);
        if args.projectile.get_entity().is_on_fire() && !properties.waterlogged && !properties.lit {
            properties.lit = true;
            args.world.set_block_state(
                args.position,
                properties.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );
        }
    }

    fn explode(&self, args: ExplodeArgs<'_>) {
        let (current_block, current_state) = args.world.get_block_and_state(args.position);
        if current_block != args.block {
            return;
        }
        let mut properties = CandleLikeProperties::from_state_id(current_state.id);
        if properties.lit {
            properties.lit = false;
            args.world.set_block_state(
                args.position,
                properties.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );
            args.world.play_sound(
                Sound::BlockCandleExtinguish,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
            );
            args.world.emit_game_event(
                GameEvent::BlockChange.name(),
                args.position.to_centered_f64(),
            );
        }
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let b = BlockAccessor::get_block(args.world, args.position);
        args.player.get_entity().pose.load() != EntityPose::Crouching
            && CandleLikeProperties::from_state_id(args.state_id).candles < 4
            && args.block.id == b.id
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let properties = CandleLikeProperties::from_state_id(args.state_id);
        if properties.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }
        args.state_id
    }
}

/// Vanilla `AbstractCandleBlock.extinguish`, for candles and candle cakes: `unlit` is the
/// block's state with `lit` cleared.
pub fn extinguish(
    world: &Arc<World>,
    position: &BlockPos,
    unlit: BlockStateId,
    source: Option<&dyn crate::entity::EntityBase>,
) {
    world.set_block_state(position, unlit, BlockFlags::NOTIFY_ALL);
    world.play_sound(
        Sound::BlockCandleExtinguish,
        SoundCategory::Blocks,
        &position.to_centered_f64(),
    );
    world.emit_game_event_with_context(
        GameEvent::BlockChange.name(),
        position.to_centered_f64(),
        &crate::world::game_event::GameEventContext {
            source_entity: source,
            affected_state: None,
        },
    );
}

fn can_place_at(block_accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    can_support_center(block_accessor, &position.down(), BlockDirection::Up)
}
