use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;

use pumpkin_data::{
    Block, BlockId, BlockStateId,
    effect::StatusEffect,
    entity::EntityType,
    particle::Particle,
    sound::{Sound, SoundCategory},
    tag::{self, Taggable},
};
use pumpkin_util::{
    Difficulty,
    math::{position::BlockPos, vector3::Vector3},
};
use pumpkin_world::{tick::TickPriority, world::BlockFlags};

use crate::{
    block::{
        BlockBehaviour, BlockMetadata, CanPlaceAtArgs, GetStateForNeighborUpdateArgs,
        OnEntityCollisionArgs, OnScheduledTickArgs, RandomTickArgs, blocks::plant::PlantBlockBase,
    },
    world::World,
};

const EYEBLOSSOM_XZ_RANGE: i32 = 3;
const EYEBLOSSOM_Y_RANGE: i32 = 2;
/// Vanilla `EyeblossomBlock.Type` particle colors.
const OPEN_PARTICLE_COLOR: i32 = 16_545_810;
const CLOSED_PARTICLE_COLOR: i32 = 6_250_335;

pub struct EyeblossomBlock;

impl EyeblossomBlock {
    /// Vanilla `EyeblossomBlock.getBeeInteractionEffect`, also what an open one gives a bee
    /// inside it.
    #[must_use]
    pub const fn bee_interaction_effect() -> pumpkin_data::potion::Effect {
        pumpkin_data::potion::Effect {
            effect_type: &StatusEffect::POISON,
            duration: 25,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: true,
        }
    }
}

impl BlockMetadata for EyeblossomBlock {
    fn ids() -> Box<[BlockId]> {
        Box::new([BlockId::OPEN_EYEBLOSSOM, BlockId::CLOSED_EYEBLOSSOM])
    }
}

impl BlockBehaviour for EyeblossomBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        <Self as PlantBlockBase>::get_state_for_neighbor_update(
            self,
            args.world,
            args.position,
            args.state_id,
        )
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let was_open = args.block == &Block::OPEN_EYEBLOSSOM;
        if try_changing_state(args.world, args.block, args.position) {
            let sound = if was_open {
                Sound::BlockEyeblossomClose
            } else {
                Sound::BlockEyeblossomOpen
            };
            args.world.play_sound(
                sound,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
            );
        }
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let was_open = args.block == &Block::OPEN_EYEBLOSSOM;
        if try_changing_state(args.world, args.block, args.position) {
            let sound = if was_open {
                Sound::BlockEyeblossomCloseLong
            } else {
                Sound::BlockEyeblossomOpenLong
            };
            args.world.play_sound(
                sound,
                SoundCategory::Blocks,
                &args.position.to_centered_f64(),
            );
        }
    }

    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        if args.world.level_info.load().difficulty == Difficulty::Peaceful {
            return;
        }

        if args.entity.get_entity().entity_type == &EntityType::BEE
            && args.block.has_tag(&tag::Block::MINECRAFT_BEE_ATTRACTIVE)
            && let Some(living_entity) = args.entity.get_living_entity()
            && !living_entity.has_effect(&StatusEffect::POISON)
        {
            living_entity.add_effect(Self::bee_interaction_effect());
        }
    }
}

impl PlantBlockBase for EyeblossomBlock {}

pub fn try_changing_state(world: &Arc<World>, current_block: &Block, pos: &BlockPos) -> bool {
    let is_open = current_block == &Block::OPEN_EYEBLOSSOM;
    let should_be_open = world.eyeblossom_open(pos).unwrap_or(is_open);

    if should_be_open == is_open {
        return false;
    }

    let new_block = if is_open {
        &Block::CLOSED_EYEBLOSSOM
    } else {
        &Block::OPEN_EYEBLOSSOM
    };

    let old_state = world.get_block_state_id(pos);
    world.set_block_state(pos, new_block.default_state.id, BlockFlags::NOTIFY_ALL);
    world.emit_game_event_with_context(
        pumpkin_data::game_event::GameEvent::BlockChange.name(),
        pos.to_centered_f64(),
        &crate::world::game_event::GameEventContext {
            source_entity: None,
            affected_state: Some(old_state),
        },
    );

    let rng = game_random();

    // Vanilla `EyeblossomBlock.Type.spawnTransformParticle`, run on the type it turned into.
    let start = pos.to_centered_f64();
    let lifetime = 0.5 + rng.random::<f64>();
    let velocity = Vector3::new(
        rng.random::<f64>() - 0.5,
        rng.random::<f64>() + 1.0,
        rng.random::<f64>() - 0.5,
    );
    let color = if is_open {
        CLOSED_PARTICLE_COLOR
    } else {
        OPEN_PARTICLE_COLOR
    };
    world.send_particles_with_data(
        Particle::Trail,
        &World::trail_particle_data(start + velocity * lifetime, color, (20.0 * lifetime) as i32),
        false,
        false,
        start,
        1,
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
    );

    for dx in -EYEBLOSSOM_XZ_RANGE..=EYEBLOSSOM_XZ_RANGE {
        for dy in -EYEBLOSSOM_Y_RANGE..=EYEBLOSSOM_Y_RANGE {
            for dz in -EYEBLOSSOM_XZ_RANGE..=EYEBLOSSOM_XZ_RANGE {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let nearby_pos = pos.offset(Vector3::new(dx, dy, dz));
                let nearby_block = world.get_block(&nearby_pos);
                if nearby_block == current_block {
                    let dist_sqr = (dx * dx + dy * dy + dz * dz) as f64;
                    let distance = dist_sqr.sqrt();
                    let min_delay = (distance * 5.0) as u32;
                    let max_delay = (distance * 10.0) as u32;
                    let delay = if min_delay >= max_delay {
                        min_delay
                    } else {
                        rng.random_range(min_delay..=max_delay)
                    };
                    world.schedule_block_tick(
                        current_block,
                        nearby_pos,
                        delay.max(1),
                        TickPriority::Normal,
                    );
                }
            }
        }
    }

    true
}
