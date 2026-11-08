use std::sync::Arc;

use pumpkin_data::entity::EntityStatus;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{BlockDirection, HorizontalFacingExt};
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::{
    MemoryModuleId, MemoryStatus,
    position_tracker::{EntityTracker, PositionTracker},
    types,
    walk_target::WalkTarget,
};
use super::Behavior;
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::custom_sound::CustomSound;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::world::World;

const TIME_OUT_DURATION: i32 = 160;

/// Vanilla `TargetingConditions` of the ram, tested with the rammer as targeter.
pub type RamTargeting = fn(&World, &dyn Mob, &dyn EntityBase) -> bool;

struct RamCandidate {
    start_position: BlockPos,
    target_position: BlockPos,
    target: Arc<dyn EntityBase>,
}

/// Walks to a spot in line with the nearest rammable entity and sets `RAM_TARGET` once it has
/// stood there for `ram_prepare_time` ticks.
pub struct PrepareRamNearestTarget {
    get_cooldown_on_fail: fn(&dyn Mob) -> i32,
    min_ram_distance: i32,
    max_ram_distance: i32,
    walk_speed: f32,
    ram_targeting: RamTargeting,
    ram_prepare_time: i64,
    get_prepare_ram_sound: fn(&dyn Mob) -> Sound,
    reached_ram_position_timestamp: Option<i64>,
    ram_candidate: Option<RamCandidate>,
}

impl PrepareRamNearestTarget {
    #[must_use]
    pub const fn new(
        get_cooldown_on_fail: fn(&dyn Mob) -> i32,
        min_ram_distance: i32,
        max_ram_distance: i32,
        walk_speed: f32,
        ram_targeting: RamTargeting,
        ram_prepare_time: i64,
        get_prepare_ram_sound: fn(&dyn Mob) -> Sound,
    ) -> Self {
        Self {
            get_cooldown_on_fail,
            min_ram_distance,
            max_ram_distance,
            walk_speed,
            ram_targeting,
            ram_prepare_time,
            get_prepare_ram_sound,
            reached_ram_position_timestamp: None,
            ram_candidate: None,
        }
    }

    fn choose_ram_position(&mut self, mob: &dyn Mob, target: Arc<dyn EntityBase>) {
        self.reached_ram_position_timestamp = None;
        self.ram_candidate = self
            .calculate_ramming_start_position(mob, target.as_ref())
            .map(|start_position| RamCandidate {
                start_position,
                target_position: target.get_entity().block_pos.load(),
                target,
            });
    }

    fn calculate_ramming_start_position(
        &self,
        mob: &dyn Mob,
        target: &dyn EntityBase,
    ) -> Option<BlockPos> {
        let target_pos = target.get_entity().block_pos.load();
        if !is_walkable_block(mob, target_pos) {
            return None;
        }
        let mut possible_ram_positions = Vec::new();
        for direction in BlockDirection::horizontal_worldgen() {
            let offset = direction.to_block_direction().to_offset();
            let mut furthest = target_pos;
            for _ in 0..self.max_ram_distance {
                furthest = furthest.offset(offset);
                if !is_walkable_block(mob, furthest) {
                    furthest = furthest.offset_dir(offset, -1);
                    break;
                }
            }
            if furthest.manhattan_distance(target_pos) >= self.min_ram_distance {
                possible_ram_positions.push(furthest);
            }
        }
        let mob_pos = mob.get_entity().block_pos.load();
        possible_ram_positions.sort_by_key(|pos| mob_pos.squared_distance(pos));
        let living = &mob.get_mob_entity().living_entity;
        possible_ram_positions.into_iter().find(|pos| {
            mob.get_mob_entity()
                .steered()
                .navigator()
                .create_path(living, pos.0.to_f64(), 0)
                .is_some_and(|path| path.can_reach())
        })
    }
}

/// Vanilla `isWalkableBlock`: a stable destination of zero malus.
fn is_walkable_block(mob: &dyn Mob, pos: BlockPos) -> bool {
    let entity = mob.get_entity();
    let world = entity.world.load_full();
    if !mob.is_stable_destination(&world, &pos) {
        return false;
    }
    let mut context = PathfindingContext::new(entity.block_pos.load().0, world);
    let path_type = context.get_land_node_type(pos.0);
    mob.get_mob_entity()
        .steered()
        .navigator()
        .get_pathfinding_malus(path_type)
        == 0.0
}

/// Vanilla `getEdgeOfBlock`: the target block's bottom center moved half a block toward the
/// start position's side.
fn get_edge_of_block(start_ram_pos: BlockPos, target_pos: BlockPos) -> Vector3<f64> {
    let x_offset = 0.5 * f64::from((target_pos.0.x - start_ram_pos.0.x).signum());
    let z_offset = 0.5 * f64::from((target_pos.0.z - start_ram_pos.0.z).signum());
    Vector3::new(
        f64::from(target_pos.0.x) + 0.5 + x_offset,
        f64::from(target_pos.0.y),
        f64::from(target_pos.0.z) + 0.5 + z_offset,
    )
}

fn broadcast_ram_status(tick: &BrainTick<'_>, status: EntityStatus) {
    tick.world
        .send_entity_status(tick.mob.get_entity(), status, ActorEventID::None);
}

impl Behavior for PrepareRamNearestTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 4] = [
            (types::LOOK_TARGET.id(), MemoryStatus::Registered),
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
            (
                types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::RAM_TARGET.id(), MemoryStatus::ValueAbsent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        TIME_OUT_DURATION
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let target = {
            let ctx = tick.visibility();
            tick.state
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .and_then(|visible| {
                    visible.find_closest(&ctx, |entity| {
                        (self.ram_targeting)(tick.world, tick.mob, entity.as_ref())
                    })
                })
        };
        if let Some(target) = target {
            self.choose_ram_position(tick.mob, target);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if !tick.state.has_memory_value(types::RAM_TARGET.id()) {
            broadcast_ram_status(tick, EntityStatus::EndRam);
            tick.state.set(
                types::RAM_COOLDOWN_TICKS,
                (self.get_cooldown_on_fail)(tick.mob),
            );
        }
    }

    fn can_still_use(&mut self, _tick: &mut BrainTick<'_>) -> bool {
        self.ram_candidate
            .as_ref()
            .and_then(|candidate| candidate.target.get_living_entity())
            .is_some_and(LivingEntity::is_alive)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(candidate) = &self.ram_candidate else {
            return;
        };
        tick.state.set(
            types::WALK_TARGET,
            WalkTarget::from_block_pos(candidate.start_position, self.walk_speed, 0),
        );
        let tracker: Arc<dyn PositionTracker> =
            Arc::new(EntityTracker::new(candidate.target.clone(), true));
        tick.state.set(types::LOOK_TARGET, tracker);
        let did_target_move =
            candidate.target.get_entity().block_pos.load() != candidate.target_position;
        if did_target_move {
            broadcast_ram_status(tick, EntityStatus::EndRam);
            tick.mob.get_mob_entity().steered().navigator().stop();
            let target = candidate.target.clone();
            self.choose_ram_position(tick.mob, target);
            return;
        }
        let start_ram_pos = tick.mob.get_entity().block_pos.load();
        if start_ram_pos != candidate.start_position {
            return;
        }
        broadcast_ram_status(tick, EntityStatus::StartRam);
        let reached = *self.reached_ram_position_timestamp.get_or_insert(tick.time);
        if tick.time - reached >= self.ram_prepare_time {
            tick.state.set(
                types::RAM_TARGET,
                get_edge_of_block(start_ram_pos, candidate.target_position),
            );
            let pitch = tick.mob.as_custom_sound().map_or_else(
                || tick.mob.get_mob_entity().living_entity.get_pitch(),
                CustomSound::voice_pitch,
            );
            tick.world.play_sound_fine(
                (self.get_prepare_ram_sound)(tick.mob),
                SoundCategory::Neutral,
                &tick.mob.get_entity().pos.load(),
                1.0,
                pitch,
            );
            self.ram_candidate = None;
        }
    }

    fn debug_name(&self) -> &'static str {
        "PrepareRamNearestTarget"
    }
}
