use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use pumpkin_data::damage::DamageType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use crate::entity::ai::goal::GoalReach;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::{Entity, EntityBase};
use crate::world::World;

const ROOSTING_FLAG: u8 = 1;
const CLOSE_PLAYER_DISTANCE: f64 = 4.0;
/// Vanilla `Bat.getSoundVolume`.
const SOUND_VOLUME: f32 = 0.1;
/// Vanilla `closerToCenterThan(position, 2.0)`.
const CLOSE_TARGET_DISTANCE_SQ: f64 = 4.0;
/// A new hanging target is at most this many blocks away on each horizontal axis.
const TARGET_RANGE: f64 = 6.0;
/// Forward input a flying bat sets each tick.
const BAT_FORWARD_INPUT: f32 = 0.5;

pub struct BatEntity {
    pub mob_entity: MobEntity,
    hanging_position: Mutex<Option<BlockPos>>,
    roosting: AtomicBool,
}

impl BatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let bat = Self {
            mob_entity,
            hanging_position: Mutex::new(None),
            roosting: AtomicBool::new(true),
        };
        Arc::new(bat)
    }

    #[must_use]
    pub fn check_bat_spawn_rules(
        world: &World,
        pos: &BlockPos,
        reason: crate::entity::mob::spawn::EntitySpawnReason,
    ) -> bool {
        if pos.0.y >= world.get_heightmap_height(ChunkHeightmapType::WorldSurface, pos.0.x, pos.0.z)
        {
            return false;
        }
        if game_random().random_bool(0.5) {
            return false;
        }
        if world.get_max_local_raw_brightness(pos) > game_random().random_range(0..4) {
            return false;
        }
        let below_pos = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        if !world
            .get_block(&below_pos)
            .has_tag(&tag::Block::MINECRAFT_BATS_SPAWNABLE_ON)
        {
            return false;
        }
        MobEntity::check_mob_spawn_rules(world, pos, reason)
    }

    #[must_use]
    pub fn is_roosting(&self) -> bool {
        self.roosting.load(Relaxed)
    }

    pub fn set_roosting(&self, roosting: bool) {
        self.roosting.store(roosting, Relaxed);
        let flags: u8 = if roosting { ROOSTING_FLAG } else { 0 };
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    /// Vanilla wakes a resting bat with level event 1025 unless it is silent.
    fn take_off(&self, world: &World, pos: BlockPos) {
        self.set_roosting(false);
        if !self.get_entity().is_silent() {
            world.sync_world_event(WorldEvent::SoundBatLiftoff, pos, 0);
        }
    }

    fn tick_roosting(&self, world: &World, block_pos: BlockPos, above_pos: &BlockPos) {
        let entity = &self.mob_entity.living_entity.entity;
        let above_state = world.get_block_state(above_pos);
        if above_state.is_solid_block() {
            let rotate_head = {
                let rng = game_random();
                (rng.random_range(0u32..200) == 0).then(|| rng.random_range(0i32..360) as f32)
            };
            if let Some(head_yaw) = rotate_head {
                entity.head_yaw.store(head_yaw);
            }

            // `BAT_RESTING_TARGETING`: a non-combat search, so spectators don't count.
            if world
                .get_nearest_player(entity.pos.load(), CLOSE_PLAYER_DISTANCE, |player| {
                    !player.is_spectator()
                })
                .is_some()
            {
                self.take_off(world, block_pos);
            }
        } else {
            self.take_off(world, block_pos);
        }
    }

    fn tick_flying(&self, world: &World, above_pos: &BlockPos, pos: &Vector3<f64>) {
        let entity = &self.mob_entity.living_entity.entity;
        let mut hanging_pos = self
            .hanging_position
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(hp) = *hanging_pos
            && (!world.get_block_state(&hp).is_air() || hp.0.y <= world.dimension.min_y)
        {
            *hanging_pos = None;
        }

        let (new_target, try_roost) = {
            let rng = game_random();
            let pick_new = hanging_pos.is_none_or(|hp| {
                rng.random_range(0..30) == 0
                    || hp.dist_to_center_sqr(pos.x, pos.y, pos.z) < CLOSE_TARGET_DISTANCE_SQ
            });
            let new_target = pick_new.then(|| {
                let x = pos.x + f64::from(rng.random_range(0..7) - rng.random_range(0..7));
                let y = pos.y + f64::from(rng.random_range(0..6)) - 2.0;
                let z = pos.z + f64::from(rng.random_range(0..7) - rng.random_range(0..7));
                BlockPos::containing(x, y, z)
            });
            (new_target, rng.random_range(0..100) == 0)
        };
        if new_target.is_some() {
            *hanging_pos = new_target;
        }

        if let Some(target) = *hanging_pos {
            let d = f64::from(target.0.x) + 0.5 - pos.x;
            let e = f64::from(target.0.y) + 0.1 - pos.y;
            let f = f64::from(target.0.z) + 0.5 - pos.z;

            // Java's `Math.signum`, which is 0 for 0 where Rust's is 1.
            let signum = |value: f64| if value == 0.0 { 0.0 } else { value.signum() };
            let velo = entity.velocity.load();
            let new_velo = Vector3::new(
                velo.x + (signum(d) * 0.5 - velo.x) * f64::from(0.1f32),
                velo.y + (signum(e) * f64::from(0.7f32) - velo.y) * f64::from(0.1f32),
                velo.z + (signum(f) * 0.5 - velo.z) * f64::from(0.1f32),
            );
            entity.velocity.store(new_velo);

            let yaw = (new_velo.z.atan2(new_velo.x) as f32).to_degrees() - 90.0;
            let yaw_diff = pumpkin_util::math::wrap_degrees(yaw - entity.yaw.load());
            // Vanilla `zza`.
            let living = &self.mob_entity.living_entity;
            let input = living.movement_input.load();
            living.movement_input.store(Vector3::new(
                input.x,
                input.y,
                f64::from(BAT_FORWARD_INPUT),
            ));
            entity.yaw.store(entity.yaw.load() + yaw_diff);
        }
        drop(hanging_pos);

        if try_roost {
            let above_state = world.get_block_state(above_pos);
            if above_state.is_solid_block() {
                self.set_roosting(true);
            }
        }
    }
}

impl Mob for BatEntity {
    fn mob_is_pushable(&self) -> bool {
        false
    }

    fn has_audited_tick(&self) -> bool {
        true
    }

    fn audited_species_reach(&self, _speed: f64) -> GoalReach {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        // The hanging target it checks for air, wherever it was picked.
        let target = self
            .hanging_position
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or(0.0, |target| {
                let center = target.to_centered_f64();
                (center.x - pos.x).abs().max((center.z - pos.z).abs()) + 1.0
            });
        GoalReach {
            // `BAT_RESTING_TARGETING` around a resting bat.
            players: if self.is_roosting() {
                CLOSE_PLAYER_DISTANCE
            } else {
                0.0
            },
            block_reads: target.max(TARGET_RANGE + 1.0),
            // A flying bat steers each axis by at most 0.05 or 0.07, and `post_tick` snaps a bat
            // that rests by the end of the tick to the ceiling, less than its height away.
            impulse: f64::from(entity.height()),
            ..GoalReach::default()
        }
    }

    fn mob_is_flapping(&self) -> bool {
        !self.is_roosting() && self.get_entity().tick_count.load(Relaxed) % 10 == 0
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flags: u8 = if self.is_roosting() { ROOSTING_FLAG } else { 0 };
        entity.set_synced_data(tracked_data::bat::DATA_ID_FLAGS, flags);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let flags: u8 = if self.is_roosting() { ROOSTING_FLAG } else { 0 };
        nbt.put_byte("BatFlags", i8::try_from(flags).unwrap_or(0));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let flags = u8::try_from(nbt.get_byte("BatFlags").unwrap_or(0)).unwrap_or(0);
        let roosting = (flags & ROOSTING_FLAG) != 0;
        self.set_roosting(roosting);
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        let mut meta = crate::entity::mob::MobMetadata::new();
        if self.is_roosting() {
            meta = meta.bat_roosting(true);
        }
        Some(meta)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_roosting() || game_random().random_range(0..4) == 0)
            .then_some(Sound::EntityBatAmbient)
    }

    // Vanilla `Bat.getVoicePitch` lowers the usual voice pitch.
    fn play_ambient_sound(&self) {
        if let Some(sound) = self.get_ambient_sound() {
            let living = &self.mob_entity.living_entity;
            if living.entity.is_silent() {
                return;
            }
            living.entity.world.load().play_sound_fine(
                sound,
                SoundCategory::Neutral,
                &living.entity.pos.load(),
                SOUND_VOLUME,
                living.get_pitch() * 0.95,
            );
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        let block_pos = entity.block_pos.load();
        let above_pos = BlockPos::new(block_pos.0.x, block_pos.0.y + 1, block_pos.0.z);
        let world = entity.world.load();
        let pos = entity.pos.load();

        if self.is_roosting() {
            self.tick_roosting(&world, block_pos, &above_pos);
        } else {
            self.tick_flying(&world, &above_pos, &pos);
        }
    }

    // Vanilla `Bat.tick`: after the normal tick and its gravity, a resting bat hangs still and a
    // flying one loses most of its vertical speed.
    fn post_tick(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if self.is_roosting() {
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            let pos = entity.pos.load();
            let snapped_y = (pos.y.floor()) + 1.0 - f64::from(entity.height());
            entity.set_pos(Vector3::new(pos.x, snapped_y, pos.z));
        } else {
            let velocity = entity.velocity.load();
            entity
                .velocity
                .store(Vector3::new(velocity.x, velocity.y * 0.6, velocity.z));
        }
    }

    // Vanilla `Bat.hurtServer` wakes a resting bat without a sound.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.is_roosting() && !self.get_entity().is_invulnerable_to(&damage_type, cause) {
            self.set_roosting(false);
        }
        true
    }
}

impl CustomSound for BatEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBatDeath)
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityBatHurt)
    }

    fn sound_volume(&self) -> f32 {
        SOUND_VOLUME
    }

    fn sound_category(&self) -> SoundCategory {
        SoundCategory::Neutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_data::entity::EntityType;

    #[tokio::test]
    async fn the_ceiling_snap_stays_within_the_audited_motion()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let start = Vector3::new(0.5, 64.95, 0.5);
        let bat = BatEntity::new(Entity::new(world.clone(), start, &EntityType::BAT));
        let reach = bat
            .get_mob_entity()
            .audited_reach(bat.as_ref())
            .ok_or("missing bat reach")?;
        bat.post_tick();
        let moved = bat.get_entity().pos.load() - start;
        world.level.shutdown().await;
        assert!(moved.y.abs() <= reach.motion.1);
        Ok(())
    }
}
