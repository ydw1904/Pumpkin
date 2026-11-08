use std::sync::Arc;

use pumpkin_data::block_properties::SnifferEggLikeProperties;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockState, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::wrap_degrees;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use uuid::Uuid;

use crate::block::{
    BlockBehaviour, OnPlaceArgs, OnScheduledTickArgs, PathComputationType, PlacedArgs,
};
use crate::entity::r#type::from_type;
use crate::world::World;

const MAX_HATCH_LEVEL: i32 = 2;
const REGULAR_HATCH_TIME_TICKS: u32 = 24000;
const BOOSTED_HATCH_TIME_TICKS: u32 = 12000;
const RANDOM_HATCH_OFFSET_TICKS: u32 = 300;
const HATCH_SOUND_VOLUME: f32 = 0.7;

#[pumpkin_block("minecraft:sniffer_egg")]
pub struct SnifferEggBlock;

impl SnifferEggBlock {
    fn hatch_boost(world: &World, pos: &BlockPos) -> bool {
        world
            .get_block(&pos.down())
            .has_tag(&tag::Block::MINECRAFT_SNIFFER_EGG_HATCH_BOOST)
    }

    /// Vanilla `SnifferEggBlock.onPlace`, which runs again for every hatch stage because each
    /// stage sets a new state.
    fn on_place(world: &Arc<World>, block: &Block, pos: BlockPos) {
        let boosted = Self::hatch_boost(world, &pos);
        if boosted {
            world.sync_world_event(WorldEvent::ParticlesEggCrack, pos, 0);
        }
        let hatch_time = if boosted {
            BOOSTED_HATCH_TIME_TICKS
        } else {
            REGULAR_HATCH_TIME_TICKS
        };
        world.emit_game_event(GameEvent::BlockPlace.name(), pos.to_centered_f64());
        let delay = hatch_time / 3 + game_random().random_range(0..RANDOM_HATCH_OFFSET_TICKS);
        world.schedule_block_tick(block, pos, delay, TickPriority::Normal);
    }

    fn play_hatch_sound(world: &World, sound: Sound, pos: &BlockPos) {
        let pitch = 0.9 + game_random().random::<f32>() * 0.2;
        world.play_sound_fine(
            sound,
            SoundCategory::Blocks,
            &pos.to_centered_f64(),
            HATCH_SOUND_VOLUME,
            pitch,
        );
    }
}

impl BlockBehaviour for SnifferEggBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let props = SnifferEggLikeProperties::default(args.block);
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        Self::on_place(args.world, args.block, *args.position);
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let world = args.world;
        let pos = *args.position;
        let mut props = SnifferEggLikeProperties::from_state_id(world.get_block_state_id(&pos));
        if i32::from(props.hatch) < MAX_HATCH_LEVEL {
            Self::play_hatch_sound(world, Sound::BlockSnifferEggCrack, &pos);
            props.hatch += 1;
            world.set_block_state(
                &pos,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
            // Pumpkin runs `placed` only for a new block, vanilla's `onPlace` for every state.
            Self::on_place(world, args.block, pos);
            return;
        }
        Self::play_hatch_sound(world, Sound::BlockSnifferEggHatch, &pos);
        // Vanilla `destroyBlock(pos, false)`: no drops, but the usual neighbor and client updates.
        world.break_block(&pos, None, BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_DROPS);
        let sniffer = from_type(
            &EntityType::SNIFFER,
            pos.to_centered_f64(),
            world,
            Uuid::new_v4(),
        );
        if let Some(mob) = sniffer.get_mob() {
            mob.spawn_as_baby();
        }
        let entity = sniffer.get_entity();
        let yaw = wrap_degrees(game_random().random::<f32>() * 360.0);
        entity.yaw.store(yaw);
        entity.head_yaw.store(yaw);
        entity.body_yaw.store(yaw);
        entity.pitch.store(0.0);
        world.spawn_entity(sniffer);
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}
