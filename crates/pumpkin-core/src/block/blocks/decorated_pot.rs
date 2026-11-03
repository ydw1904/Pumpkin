use std::sync::Arc;

use pumpkin_data::block_properties::DecoratedPotLikeProperties;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::{BlockState, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;

use crate::block::entities::decorated_pot::DecoratedPotBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, GetComparatorOutputArgs, GetStateForNeighborUpdateArgs, NormalUseArgs,
    OnPlaceArgs, OnSyncedBlockEventArgs, PathComputationType, PlacedArgs, UseWithItemArgs,
};

/// Block event that makes the pot wobble; the data is the `WobbleStyle` ordinal.
const WOBBLE_EVENT: u8 = 1;
const WOBBLE_POSITIVE: u8 = 0;
const WOBBLE_NEGATIVE: u8 = 1;

#[pumpkin_block("minecraft:decorated_pot")]
pub struct DecoratedPotBlock;

impl BlockBehaviour for DecoratedPotBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = DecoratedPotLikeProperties::from_state_id(args.block.default_state.id);
        props.facing = args.player.living_entity.entity.get_horizontal_facing();
        props.waterlogged = args.replacing.water_source();
        props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        let props = DecoratedPotLikeProperties::from_state_id(args.state_id);
        if props.waterlogged {
            args.world.schedule_fluid_tick(
                &Fluid::WATER,
                *args.position,
                Fluid::WATER.flow_speed,
                TickPriority::Normal,
            );
        }
        args.state_id
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        let entity = DecoratedPotBlockEntity::new(*args.position);
        args.world.add_block_entity(Arc::new(entity));
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        if args.item_stack.item_count == 0 {
            return self.normal_use(NormalUseArgs {
                server: args.server,
                world: args.world,
                block: args.block,
                position: args.position,
                player: args.player,
                hit: args.hit,
            });
        }

        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(pot_entity) = block_entity
                .as_any()
                .downcast_ref::<DecoratedPotBlockEntity>()
        {
            let item = args.item_stack.item;
            let Some(fill) =
                pot_entity.try_insert_item(args.item_stack, args.player.gamemode.load())
            else {
                // TRY_WITH_EMPTY_HAND
                return self.normal_use(NormalUseArgs {
                    server: args.server,
                    world: args.world,
                    block: args.block,
                    position: args.position,
                    player: args.player,
                    hit: args.hit,
                });
            };
            args.world
                .add_synced_block_event(*args.position, WOBBLE_EVENT, WOBBLE_POSITIVE);
            args.player
                .increment_stat(StatisticCategory::Used, i32::from(item.id), 1);
            args.world.play_sound_raw(
                Sound::BlockDecoratedPotInsert as u16,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
                1.0,
                0.7 + 0.5 * fill,
            );
            args.world.spawn_particle(
                args.position.to_f64() + Vector3::new(0.5, 1.2, 0.5),
                Vector3::new(0.0, 0.0, 0.0),
                0.0,
                7,
                Particle::DustPlume,
            );
            // Vanilla `BlockEntity.setChanged` notifies comparators.
            args.world
                .update_neighbour_for_output_signal(args.position, args.block);
            args.world.emit_game_event(
                GameEvent::BlockChange.name(),
                args.position.to_centered_f64(),
            );
            return BlockActionResult::Success;
        }

        BlockActionResult::Pass
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        args.world.play_sound(
            Sound::BlockDecoratedPotInsertFail,
            SoundCategory::Blocks,
            &args.position.to_centered_f64(),
        );
        args.world
            .add_synced_block_event(*args.position, WOBBLE_EVENT, WOBBLE_NEGATIVE);
        args.world.emit_game_event(
            GameEvent::BlockChange.name(),
            args.position.to_centered_f64(),
        );
        BlockActionResult::Success
    }

    /// `DecoratedPotBlockEntity.triggerEvent`: the client plays the wobble.
    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        args.r#type == WOBBLE_EVENT
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(pot_entity) = block_entity
                .as_any()
                .downcast_ref::<DecoratedPotBlockEntity>()
        {
            Some(pot_entity.get_comparator_output())
        } else {
            Some(0)
        }
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}
