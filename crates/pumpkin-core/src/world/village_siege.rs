use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::TIMELINES;
use pumpkin_data::tag::Taggable;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_WITHOUT_ZOMBIE_SIEGES;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::chunk::ChunkHeightmapType;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::spawn::{EntitySpawnReason, finalize_spawn};
use crate::entity::r#type::{check_spawn_rules, from_type};
use crate::world::World;

/// Vanilla `ClockTimeMarkers.ROLL_VILLAGE_SIEGE`.
const ROLL_VILLAGE_SIEGE: &str = "minecraft:roll_village_siege";
const SIEGE_ZOMBIES: i32 = 20;
const SIEGE_RADIUS: f32 = 32.0;

#[derive(PartialEq, Eq, Clone, Copy)]
enum State {
    SiegeTonight,
    SiegeDone,
}

/// Vanilla `VillageSiege`: on one night in ten, twenty zombies spawn around a village that a
/// player is in.
pub struct VillageSiege {
    has_setup_siege: bool,
    siege_state: State,
    zombies_to_spawn: i32,
    next_spawn_time: i32,
    spawn_pos: BlockPos,
}

impl Default for VillageSiege {
    fn default() -> Self {
        Self {
            has_setup_siege: false,
            siege_state: State::SiegeDone,
            zombies_to_spawn: 0,
            next_spawn_time: 0,
            spawn_pos: BlockPos::new(0, 0, 0),
        }
    }
}

impl VillageSiege {
    pub fn tick(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if world.is_bright_outside() || !spawn_enemies {
            self.siege_state = State::SiegeDone;
            self.has_setup_siege = false;
            return;
        }
        if is_at_time_marker(world, ROLL_VILLAGE_SIEGE) {
            self.siege_state = if game_random().random_range(0..10) == 0 {
                State::SiegeTonight
            } else {
                State::SiegeDone
            };
        }
        if self.siege_state == State::SiegeDone {
            return;
        }
        if !self.has_setup_siege {
            if !self.try_to_setup_siege(world) {
                return;
            }
            self.has_setup_siege = true;
        }
        if self.next_spawn_time > 0 {
            self.next_spawn_time -= 1;
            return;
        }
        self.next_spawn_time = 2;
        if self.zombies_to_spawn > 0 {
            self.try_spawn(world);
            self.zombies_to_spawn -= 1;
        } else {
            self.siege_state = State::SiegeDone;
        }
    }

    fn try_to_setup_siege(&mut self, world: &Arc<World>) -> bool {
        let random = game_random();
        for player in world.players.load().iter() {
            if player.is_spectator() {
                continue;
            }
            let center = player.get_entity().block_pos.load();
            if !world.is_village(&center)
                || world
                    .get_biome(&center)
                    .has_tag(&MINECRAFT_WITHOUT_ZOMBIE_SIEGES)
            {
                continue;
            }
            for _ in 0..10 {
                let angle = random.random::<f32>() * std::f32::consts::TAU;
                self.spawn_pos = BlockPos::new(
                    center.0.x + (angle.cos() * SIEGE_RADIUS).floor() as i32,
                    center.0.y,
                    center.0.z + (angle.sin() * SIEGE_RADIUS).floor() as i32,
                );
                if find_random_spawn_pos(world, self.spawn_pos).is_some() {
                    self.next_spawn_time = 0;
                    self.zombies_to_spawn = SIEGE_ZOMBIES;
                    break;
                }
            }
            return true;
        }
        false
    }

    fn try_spawn(&self, world: &Arc<World>) {
        let Some(spawn_pos) = find_random_spawn_pos(world, self.spawn_pos) else {
            return;
        };
        // Vanilla finalizes the zombie before placing it, so it still stands at the origin.
        let zombie = from_type(
            &EntityType::ZOMBIE,
            Vector3::default(),
            world,
            Uuid::new_v4(),
        );
        finalize_spawn(&zombie, world, EntitySpawnReason::Event, None);
        let entity = zombie.get_entity();
        entity.set_pos(spawn_pos);
        entity.set_rotation(game_random().random::<f32>() * 360.0, 0.0);
        world.spawn_entity(zombie);
    }
}

/// Vanilla `VillageSiege.findRandomSpawnPos`.
fn find_random_spawn_pos(world: &World, pos: BlockPos) -> Option<Vector3<f64>> {
    let random = game_random();
    for _ in 0..10 {
        let x = pos.0.x + random.random_range(0..16) - 8;
        let z = pos.0.z + random.random_range(0..16) - 8;
        let y = world.get_heightmap_height(ChunkHeightmapType::WorldSurface, x, z);
        let offset = BlockPos::new(x, y, z);
        if world.is_village(&offset)
            && check_spawn_rules(
                &EntityType::ZOMBIE,
                world,
                &offset,
                world.is_thundering(),
                EntitySpawnReason::Event,
            )
        {
            return Some(Vector3::new(
                f64::from(x) + 0.5,
                f64::from(y),
                f64::from(z) + 0.5,
            ));
        }
    }
    None
}

/// Vanilla `ClockManager.isAtTimeMarker` on the dimension's default clock.
fn is_at_time_marker(world: &World, marker: &str) -> bool {
    let Some(clock) = world.dimension.default_clock else {
        return false;
    };
    let ticks = world.get_time_of_day();
    TIMELINES
        .iter()
        .filter(|timeline| timeline.clock == clock)
        .any(|timeline| {
            // Vanilla `ClockTimeMarker.occursAt`.
            let ticks = timeline
                .period_ticks
                .map_or(ticks, |period| ticks.rem_euclid(i64::from(period)));
            timeline.time_markers.iter().any(|time_marker| {
                time_marker.name == marker && ticks == i64::from(time_marker.ticks)
            })
        })
}
