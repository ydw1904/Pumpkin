use std::sync::Arc;

use pumpkin_data::block_properties::{
    Axis, CreakingHeartLikeProperties, CreakingHeartState, PaleOakWoodLikeProperties,
};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::block::entities::creaking_heart::CreakingHeartBlockEntity;
use crate::block::{
    BlockBehaviour, GetComparatorOutputArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::player::Player;
use crate::world::World;

#[pumpkin_block("minecraft:creaking_heart")]
pub struct CreakingHeartBlock;

impl CreakingHeartBlock {
    /// Vanilla `hasRequiredLogs`: pale oak logs along the heart's axis on both sides.
    pub fn has_required_logs(world: &dyn BlockAccessor, pos: &BlockPos, axis: Axis) -> bool {
        let neighbours = match axis {
            Axis::X => [pos.west(), pos.east()],
            Axis::Y => [pos.down(), pos.up()],
            Axis::Z => [pos.north(), pos.south()],
        };
        neighbours.iter().all(|neighbour| {
            let state_id = world.get_block_state_id(neighbour);
            state_id
                .to_block()
                .has_tag(&tag::Block::MINECRAFT_PALE_OAK_LOGS)
                && PaleOakWoodLikeProperties::from_state_id(state_id).axis == axis
        })
    }

    /// Vanilla `updateState`: an uprooted heart between logs wakes up, or goes dormant by day.
    fn update_state(world: &World, pos: &BlockPos, state_id: BlockStateId) -> BlockStateId {
        let mut props = CreakingHeartLikeProperties::from_state_id(state_id);
        if props.creaking_heart_state == CreakingHeartState::Uprooted
            && Self::has_required_logs(world, pos, props.axis)
        {
            props.creaking_heart_state = if world.creaking_active(pos) {
                CreakingHeartState::Awake
            } else {
                CreakingHeartState::Dormant
            };
        }
        props.to_state_id(&Block::CREAKING_HEART)
    }

    /// Vanilla `playerWillDestroy`: the protector dies by the player's hand.
    pub fn player_will_destroy(world: &Arc<World>, pos: &BlockPos, player: &Arc<Player>) {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(heart) = block_entity
            .as_any()
            .downcast_ref::<CreakingHeartBlockEntity>()
        else {
            return;
        };
        heart.remove_protector(world, Some(player));
        Self::try_award_experience(world, pos, player);
    }

    /// Vanilla `tryAwardExperience`.
    fn try_award_experience(world: &Arc<World>, pos: &BlockPos, player: &Player) {
        let props = CreakingHeartLikeProperties::from_state_id(world.get_block_state_id(pos));
        let game_mode = player.gamemode.load();
        if game_mode != GameMode::Creative
            && game_mode != GameMode::Spectator
            && props.natural
            && world.level_info.load().game_rules.block_drops
        {
            let amount = game_random().random_range(20..=24);
            ExperienceOrbEntity::spawn(world, pos.to_centered_f64(), amount);
        }
    }
}

impl BlockBehaviour for CreakingHeartBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = CreakingHeartLikeProperties::default(args.block);
        props.axis = match args.direction {
            pumpkin_data::BlockDirection::North | pumpkin_data::BlockDirection::South => Axis::Z,
            pumpkin_data::BlockDirection::East | pumpkin_data::BlockDirection::West => Axis::X,
            pumpkin_data::BlockDirection::Up | pumpkin_data::BlockDirection::Down => Axis::Y,
        };
        Self::update_state(args.world, args.position, props.to_state_id(args.block))
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        args.world
            .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        args.state_id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let new_state_id = Self::update_state(args.world, args.position, state_id);
        if new_state_id != state_id {
            args.world
                .set_block_state(args.position, new_state_id, BlockFlags::NOTIFY_ALL);
        }
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        if CreakingHeartLikeProperties::from_state_id(args.state.id).creaking_heart_state
            == CreakingHeartState::Uprooted
        {
            return Some(0);
        }
        Some(
            args.world
                .get_block_entity(args.position)
                .and_then(|block_entity| {
                    block_entity
                        .as_any()
                        .downcast_ref::<CreakingHeartBlockEntity>()
                        .map(CreakingHeartBlockEntity::get_analog_output_signal)
                })
                .unwrap_or(0),
        )
    }
}
