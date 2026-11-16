use crate::block::entities::BlockEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::memory::types;
use crate::entity::mob::raider::is_raider;
use crate::world::World;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use std::any::Any;
use std::sync::{Arc, Mutex};

/// Vanilla `BellBlockEntity.DURATION`: how long a ring shakes the bell.
const DURATION: i32 = 50;
const GLOW_DURATION: i32 = 60;
const MIN_TICKS_BETWEEN_SEARCHES: i64 = 60;
const MAX_RESONATION_TICKS: i32 = 40;
const TICKS_BEFORE_RESONATION: i32 = 5;
const SEARCH_RADIUS: f64 = 48.0;
const HEAR_BELL_RADIUS: f64 = 32.0;
const HIGHLIGHT_RAIDERS_RADIUS: f64 = 48.0;

pub struct BellBlockEntity {
    pub position: BlockPos,
    pub last_side_hit: AtomicCell<Option<HorizontalFacing>>,
    pub ring_ticks: AtomicCell<i32>,
    pub ringing: AtomicCell<bool>,
    resonating: AtomicCell<bool>,
    resonate_time: AtomicCell<i32>,
    last_ring_timestamp: AtomicCell<i64>,
    /// The living entities found around the bell by the last ring, vanilla `nearbyEntities`.
    nearby_entities: Mutex<Option<Vec<Arc<dyn EntityBase>>>>,
}

impl BellBlockEntity {
    pub const ID: &'static str = "minecraft:bell";
    /// The block event a ring sends, vanilla `triggerEvent(1, direction)`.
    pub const RING_EVENT: u8 = 1;

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            last_side_hit: AtomicCell::new(None),
            ring_ticks: AtomicCell::new(0),
            resonate_time: AtomicCell::new(0),
            resonating: AtomicCell::new(false),
            ringing: AtomicCell::new(false),
            last_ring_timestamp: AtomicCell::new(0),
            nearby_entities: Mutex::new(None),
        }
    }

    /// Vanilla `onHit`; the ring block event follows.
    pub fn activate(&self, direction: HorizontalFacing) {
        self.last_side_hit.store(Some(direction));
        if self.ringing.load() {
            self.ring_ticks.store(0);
        } else {
            self.ringing.store(true);
        }
    }

    /// Vanilla `triggerEvent` for the ring event: find who is around and tell them the bell rang.
    pub fn trigger_ring(&self, world: &World, direction: Option<HorizontalFacing>) {
        self.update_entities(world);
        self.resonate_time.store(0);
        if direction.is_some() {
            self.last_side_hit.store(direction);
        }
        self.ring_ticks.store(0);
        self.ringing.store(true);
    }

    fn closer_to_center_than(&self, entity: &dyn EntityBase, distance: f64) -> bool {
        self.position
            .to_centered_f64()
            .squared_distance_to_vec(&entity.get_entity().pos.load())
            < distance * distance
    }

    fn is_present(entity: &dyn EntityBase) -> bool {
        entity
            .get_living_entity()
            .is_some_and(crate::entity::living::LivingEntity::is_alive)
    }

    /// Vanilla `updateEntities`: the living entities within 48 blocks are looked up again at
    /// most every 60 ticks, and those within 32 remember the time they heard the bell.
    fn update_entities(&self, world: &World) {
        let game_time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .query_gametime();
        let mut nearby = self
            .nearby_entities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if game_time > self.last_ring_timestamp.load() + MIN_TICKS_BETWEEN_SEARCHES
            || nearby.is_none()
        {
            self.last_ring_timestamp.store(game_time);
            let area = BoundingBox::from_block(&self.position).expand_all(SEARCH_RADIUS);
            *nearby = Some(
                world
                    .get_entities_and_players_at_box(&area)
                    .into_iter()
                    .filter(|entity| entity.get_living_entity().is_some())
                    .collect(),
            );
        }
        for entity in nearby.iter().flatten() {
            if Self::is_present(entity.as_ref())
                && self.closer_to_center_than(entity.as_ref(), HEAR_BELL_RADIUS)
                && let Some(mob) = entity.get_mob()
            {
                mob.get_mob_entity()
                    .set_existing_brain_memory(types::HEARD_BELL_TIME, game_time);
            }
        }
    }

    /// Vanilla `areRaidersNearby`: a raider within 32 blocks makes the bell resonate.
    fn are_raiders_nearby(&self) -> bool {
        self.nearby_entities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .flatten()
            .any(|entity| {
                Self::is_present(entity.as_ref())
                    && self.closer_to_center_than(entity.as_ref(), HEAR_BELL_RADIUS)
                    && is_raider(entity.get_entity().entity_type)
            })
    }

    /// Vanilla `makeRaidersGlow`: once the resonance ends, raiders within 48 blocks glow.
    fn make_raiders_glow(&self) {
        let nearby = self
            .nearby_entities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for entity in nearby.iter().flatten() {
            if Self::is_present(entity.as_ref())
                && self.closer_to_center_than(entity.as_ref(), HIGHLIGHT_RAIDERS_RADIUS)
                && is_raider(entity.get_entity().entity_type)
                && let Some(living) = entity.get_living_entity()
            {
                living.add_effect(Effect {
                    effect_type: &StatusEffect::GLOWING,
                    duration: GLOW_DURATION,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
        }
    }
}

impl BlockEntity for BellBlockEntity {
    fn write_nbt(&self, _nbt: &mut NbtCompound) {}

    fn from_nbt(_nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self::new(position)
    }

    fn tick(&self, world: &Arc<World>) {
        if self.ringing.load() {
            self.ring_ticks.fetch_add(1);
        }
        if self.ring_ticks.load() >= DURATION {
            self.ringing.store(false);
            self.ring_ticks.store(0);
        }
        if self.ring_ticks.load() >= TICKS_BEFORE_RESONATION
            && self.resonate_time.load() == 0
            && self.are_raiders_nearby()
        {
            self.resonating.store(true);
            world.play_sound_fine(
                Sound::BlockBellResonate,
                SoundCategory::Blocks,
                &self.position.to_f64(),
                1.0,
                1.0,
            );
        }

        if self.resonating.load() {
            if self.resonate_time.load() < MAX_RESONATION_TICKS {
                self.resonate_time.fetch_add(1);
            } else {
                self.make_raiders_glow();
                self.resonating.store(false);
            }
        }
    }

    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn bedrock_block_actor_data(
        &self,
        _state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let direction = match self.last_side_hit.load() {
            Some(HorizontalFacing::South) => 0,
            Some(HorizontalFacing::West) => 1,
            Some(HorizontalFacing::North) => 2,
            Some(HorizontalFacing::East) => 3,
            None => 255,
        };
        Some(
            super::bedrock::BedrockBlockActorBuilder::new("Bell", self.position)
                .movable(false)
                .put_bool("Ringing", self.ringing.load())
                .put_int("Ticks", self.ring_ticks.load())
                .put_int("Direction", direction)
                .finish(),
        )
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
