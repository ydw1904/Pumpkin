use std::sync::Arc;

use pumpkin_data::block_properties::DecoratedPotLikeProperties;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::block::entities::decorated_pot::DecoratedPotBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, GetComparatorOutputArgs, GetStateForNeighborUpdateArgs, NormalUseArgs,
    OnPlaceArgs, OnProjectileHitArgs, OnStateReplacedArgs, OnSyncedBlockEventArgs,
    PathComputationType, PlacedArgs, UseWithItemArgs,
};
use crate::entity::player::Player;
use crate::entity::projectile::{may_break, may_interact};
use crate::world::World;

/// Block event that makes the pot wobble; the data is the `WobbleStyle` ordinal.
const WOBBLE_EVENT: u8 = 1;
const WOBBLE_POSITIVE: u8 = 0;
const WOBBLE_NEGATIVE: u8 = 1;

#[pumpkin_block("minecraft:decorated_pot")]
pub struct DecoratedPotBlock;

impl DecoratedPotBlock {
    pub fn player_will_destroy(
        world: &Arc<World>,
        position: &BlockPos,
        broken_block: &Block,
        state: &'static BlockState,
        cause: Option<&Arc<Player>>,
    ) -> &'static BlockState {
        let Some(player) = cause.filter(|_| broken_block == &Block::DECORATED_POT) else {
            return state;
        };
        let destroyed_with = player.inventory().held_item();
        if destroyed_with
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_BREAKS_DECORATED_POTS)
            && !destroyed_with.has_enchantment_tag(
                &pumpkin_data::tag::Enchantment::MINECRAFT_PREVENTS_DECORATED_POT_SHATTERING,
            )
        {
            let mut props = DecoratedPotLikeProperties::from_state_id(state.id);
            props.cracked = true;
            let cracked_id = props.to_state_id(&Block::DECORATED_POT);
            world.set_block_state(
                position,
                cracked_id,
                BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK,
            );
            return BlockState::from_id(cracked_id);
        }
        state
    }
}

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
            args.world.emit_game_event_with_context(
                GameEvent::BlockChange.name(),
                args.position.to_centered_f64(),
                &crate::world::game_event::GameEventContext {
                    source_entity: Some(args.player.as_ref()),
                    affected_state: None,
                },
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
        args.world.emit_game_event_with_context(
            GameEvent::BlockChange.name(),
            args.position.to_centered_f64(),
            &crate::world::game_event::GameEventContext {
                source_entity: Some(args.player.as_ref()),
                affected_state: None,
            },
        );
        BlockActionResult::Success
    }

    /// `DecoratedPotBlockEntity.triggerEvent`: the client plays the wobble.
    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        args.r#type == WOBBLE_EVENT
    }

    fn on_projectile_hit(&self, args: OnProjectileHitArgs<'_>) {
        if may_interact(args.projectile, args.world, args.position)
            && may_break(args.projectile, args.world)
        {
            let mut props = DecoratedPotLikeProperties::from_state_id(args.state.id);
            props.cracked = true;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK,
            );
            args.world
                .break_block(args.position, None, BlockFlags::NOTIFY_ALL);
        }
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        args.world
            .update_neighbour_for_output_signal(args.position, args.block);
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
