use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pumpkin_data::block_properties::SculkCatalystLikeProperties;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_world::generation::feature::features::sculk::SculkLevel;
use pumpkin_world::generation::feature::features::sculk::spreader::{
    ChargeCursor, MAX_CHARGE, MAX_CURSORS, SculkSpreader,
};
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use super::BlockEntity;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::world::World;
use crate::world::game_event::{GameEventContext, GameEventListener};

/// Vanilla `SculkCatalystBlockEntity.CatalystListener.PULSE_TICKS`.
const PULSE_TICKS: u32 = 8;
/// Vanilla `CatalystListener.getListenerRadius`.
const LISTENER_RADIUS: i32 = 8;
const DIRECTION_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// Vanilla `SculkCatalystBlockEntity` with its `CatalystListener`.
pub struct SculkCatalystBlockEntity {
    pub position: BlockPos,
    spreader: Mutex<SculkSpreader>,
    /// A death was heard since the last tick. The bloom waits for the block entity tick, so
    /// hearing a death never changes blocks from inside another entity's tick.
    bloom_pending: AtomicBool,
}

impl BlockEntity for SculkCatalystBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let mut spreader = SculkSpreader::new_level_spreader();
        if let Some(cursors) = nbt.get_list("cursors").and_then(read_cursors) {
            for cursor in cursors {
                spreader.load_cursor(cursor);
            }
        }
        Self {
            position,
            spreader: Mutex::new(spreader),
            bloom_pending: AtomicBool::new(false),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        let spreader = self.spreader.lock().unwrap_or_else(PoisonError::into_inner);
        nbt.put_list(
            "cursors",
            spreader
                .cursors()
                .iter()
                .map(|cursor| NbtTag::Compound(write_cursor(cursor)))
                .collect(),
        );
    }

    /// Vanilla `SculkCatalystBlockEntity.serverTick`.
    fn tick(&self, world: &Arc<World>) {
        if self.bloom_pending.swap(false, Ordering::Relaxed) {
            self.bloom(world);
        }
        let mut spreader = self.spreader.lock().unwrap_or_else(PoisonError::into_inner);
        if spreader.cursors().is_empty() {
            return;
        }
        let mut random =
            RandomGenerator::Xoroshiro(Xoroshiro::from_seed(game_random().random::<u64>()));
        spreader.update_cursors(
            &mut WorldSculkLevel { world },
            self.position,
            &mut random,
            true,
        );
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn game_event_listener(&self) -> Option<&dyn GameEventListener> {
        Some(self)
    }
}

impl SculkCatalystBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_catalyst";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            spreader: Mutex::new(SculkSpreader::new_level_spreader()),
            bloom_pending: AtomicBool::new(false),
        }
    }

    /// Vanilla `CatalystListener.bloom`.
    fn bloom(&self, world: &Arc<World>) {
        let state = world.get_block_state_id(&self.position);
        if state.to_block_id() != BlockId::SCULK_CATALYST {
            return;
        }
        let block = &Block::SCULK_CATALYST;
        let mut props = SculkCatalystLikeProperties::from_state_id(state);
        props.bloom = true;
        world.set_block_state(
            &self.position,
            props.to_state_id(block),
            BlockFlags::NOTIFY_ALL,
        );
        world.schedule_block_tick(block, self.position, PULSE_TICKS, TickPriority::Normal);
        world.spawn_particles(
            Particle::SculkSoul,
            Vector3::new(
                f64::from(self.position.0.x) + 0.5,
                f64::from(self.position.0.y) + 1.15,
                f64::from(self.position.0.z) + 0.5,
            ),
            2,
            Vector3::new(0.2, 0.0, 0.2),
            0.0,
        );
        let pitch = game_random().random::<f32>().mul_add(0.4, 0.6);
        world.play_sound_fine(
            Sound::BlockSculkCatalystBloom,
            SoundCategory::Blocks,
            &self.position.to_centered_f64(),
            2.0,
            pitch,
        );
    }
}

impl GameEventListener for SculkCatalystBlockEntity {
    fn listener_position(&self) -> Vector3<f64> {
        self.position.to_centered_f64()
    }

    fn listener_radius(&self) -> i32 {
        LISTENER_RADIUS
    }

    fn delivered_by_distance(&self) -> bool {
        true
    }

    fn handle_game_event(
        &self,
        world: &World,
        event: GameEvent,
        context: &GameEventContext<'_>,
        source_position: Vector3<f64>,
    ) -> bool {
        if event != GameEvent::EntityDie {
            return false;
        }
        let Some(mob) = context.source_entity else {
            return false;
        };
        let Some(living) = mob.get_living_entity() else {
            return false;
        };
        if living.was_experience_consumed() {
            return true;
        }
        let killer = living
            .get_last_damage_source()
            .and_then(|(_, attacker)| (attacker != 0).then(|| world.get_entity_by_id(attacker)))
            .flatten();
        let experience = mob.get_experience_reward(killer.as_deref());
        if mob.should_drop_experience() && experience > 0 {
            self.spreader
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .add_cursors(
                    BlockPos::floored_v(source_position + Vector3::new(0.0, 0.5, 0.0)),
                    experience as i32,
                );
            // Vanilla `tryAwardItSpreadsAdvancement`.
            if let Some(player) = living
                .get_last_hurt_by_mob()
                .and_then(|attacker| world.get_player_by_id(attacker.get_entity().entity_id))
            {
                player.trigger_advancement(AdvancementTrigger::KillMobNearSculkCatalyst);
            }
        }
        living.skip_drop_experience();
        self.bloom_pending.store(true, Ordering::Relaxed);
        true
    }
}

fn write_cursor(cursor: &ChargeCursor) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.put(
        "pos",
        NbtTag::IntArray(vec![cursor.pos.0.x, cursor.pos.0.y, cursor.pos.0.z]),
    );
    nbt.put_int("charge", i32::from(cursor.charge));
    nbt.put_int("decay_delay", i32::from(cursor.decay_delay));
    nbt.put_int("update_delay", i32::from(cursor.update_delay));
    if let Some(faces) = cursor.faces {
        nbt.put_list(
            "facings",
            BlockDirection::all()
                .into_iter()
                .filter(|direction| faces & (1 << direction.to_index()) != 0)
                .map(|direction| {
                    NbtTag::String(DIRECTION_NAMES[direction.to_index() as usize].into())
                })
                .collect(),
        );
    }
    nbt
}

/// Vanilla `SculkSpreader.load`: a list of more than [`MAX_CURSORS`] cursors, or any invalid
/// cursor, loads none.
fn read_cursors(list: &[NbtTag]) -> Option<Vec<ChargeCursor>> {
    if list.len() > MAX_CURSORS {
        return None;
    }
    list.iter()
        .map(|tag| {
            let nbt = tag.extract_compound()?;
            let pos = nbt.get_int_array("pos")?;
            let [x, y, z] = *pos else {
                return None;
            };
            let charge = nbt.get_int("charge").unwrap_or(0);
            let decay_delay = nbt.get_int("decay_delay").unwrap_or(1);
            let update_delay = nbt.get_int("update_delay").unwrap_or(0);
            if !(0..=i32::from(MAX_CHARGE)).contains(&charge)
                || !(0..=1).contains(&decay_delay)
                || update_delay < 0
            {
                return None;
            }
            // Facings are lenient: unreadable ones load as none.
            let faces = nbt.get_list("facings").and_then(|facings| {
                facings.iter().try_fold(0u8, |faces, facing| {
                    let index = DIRECTION_NAMES
                        .iter()
                        .position(|name| Some(*name) == facing.extract_string())?;
                    Some(faces | 1 << index)
                })
            });
            let mut cursor = ChargeCursor::new(BlockPos::new(x, y, z), charge as u16);
            cursor.decay_delay = decay_delay as u8;
            cursor.update_delay = update_delay.min(i32::from(u8::MAX)) as u8;
            cursor.faces = faces;
            Some(cursor)
        })
        .collect()
}

/// The world as the level a catalyst's cursors spread sculk in.
struct WorldSculkLevel<'a> {
    world: &'a Arc<World>,
}

impl SculkLevel for WorldSculkLevel<'_> {
    fn sculk_get(&self, pos: BlockPos) -> Option<BlockStateId> {
        Some(self.world.get_block_state_id(&pos))
    }

    fn sculk_set(&mut self, pos: BlockPos, state: &'static BlockState) {
        self.world
            .set_block_state(&pos, state.id, BlockFlags::NOTIFY_ALL);
    }

    fn sculk_set_spread(&mut self, pos: BlockPos, state: &'static BlockState) {
        // Vanilla `MultifaceSpreader.spreadToFace` sets with flag 2.
        self.world
            .set_block_state(&pos, state.id, BlockFlags::NOTIFY_LISTENERS);
    }

    fn sculk_is_air(&self, pos: BlockPos) -> bool {
        self.world.get_block_state(&pos).is_air()
    }

    fn sculk_is_water_source(&self, pos: BlockPos) -> bool {
        let (fluid, state) = self.world.get_fluid_and_fluid_state(&pos);
        fluid == &Fluid::WATER && state.is_source
    }

    fn sculk_is_water(&self, pos: BlockPos) -> bool {
        self.world.get_fluid(&pos) == &Fluid::WATER
    }

    fn sculk_is_face_sturdy(&self, pos: BlockPos, face: BlockDirection) -> bool {
        self.world.get_block_state(&pos).is_side_solid(face)
    }

    fn sculk_is_full_cube(&self, pos: BlockPos) -> bool {
        self.world.get_block_state(&pos).is_full_cube()
    }

    fn sculk_spread_veins(&mut self, pos: BlockPos) {
        self.world.play_sound(
            Sound::BlockSculkSpread,
            SoundCategory::Blocks,
            &pos.to_centered_f64(),
        );
    }

    fn sculk_placed_sculk(&mut self, pos: BlockPos, replaced: BlockStateId) {
        crate::block::push_entities_up(
            self.world,
            BlockState::from_id(replaced),
            Block::SCULK.default_state.id,
            &pos,
        );
        self.sculk_spread_veins(pos);
    }

    fn sculk_placed_growth(&mut self, charge_pos: BlockPos, growth: &'static BlockState) {
        let sound = if growth.id.to_block_id() == BlockId::SCULK_SHRIEKER {
            Sound::BlockSculkShriekerPlace
        } else {
            Sound::BlockSculkSensorPlace
        };
        self.world
            .play_sound(sound, SoundCategory::Blocks, &charge_pos.to_centered_f64());
    }

    fn sculk_charge(&mut self, pos: BlockPos, data: i32) {
        self.world
            .sync_world_event(WorldEvent::ParticlesSculkCharge, pos, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vanilla_cursor(charge: i32, facings: &[&str]) -> NbtTag {
        let mut nbt = NbtCompound::new();
        nbt.put("pos", NbtTag::IntArray(vec![1, -2, 3]));
        nbt.put_int("charge", charge);
        nbt.put_int("decay_delay", 0);
        nbt.put_int("update_delay", 1);
        nbt.put_list(
            "facings",
            facings
                .iter()
                .map(|facing| NbtTag::String((*facing).into()))
                .collect(),
        );
        NbtTag::Compound(nbt)
    }

    #[test]
    fn reads_vanilla_cursors() {
        let cursors = read_cursors(&[vanilla_cursor(5, &["down", "east"])]).unwrap_or_default();
        assert_eq!(cursors.len(), 1);
        let cursor = &cursors[0];
        assert_eq!(cursor.pos, BlockPos::new(1, -2, 3));
        assert_eq!(
            (cursor.charge, cursor.decay_delay, cursor.update_delay),
            (5, 0, 1)
        );
        assert_eq!(cursor.faces, Some(0b10_0001));
        assert!(read_cursors(&[vanilla_cursor(1001, &[])]).is_none());
        assert!(read_cursors(&vec![vanilla_cursor(1, &[]); MAX_CURSORS + 1]).is_none());
    }
}
