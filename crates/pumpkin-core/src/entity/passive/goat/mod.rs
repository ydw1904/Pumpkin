mod ai;
mod ram_target;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::InstrumentImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomImpl, legacy_rand::LegacyRand};

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::brain::{Brain, memory::PackedMemories},
    ai::pathfinder::node::PathType,
    custom_sound::CustomSound,
    item::ItemEntity,
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::animal::Animal,
    player::Player,
};
use crate::item::items::bucket::fill_bucket_stack;
use crate::world::World;

const ADULT_ATTACK_DAMAGE: f64 = 2.0;
const BABY_ATTACK_DAMAGE: f64 = 1.0;
const GOAT_FALL_DAMAGE_REDUCTION: i32 = 10;
const GOAT_SCREAMING_CHANCE: f64 = 0.02;
const UNIHORN_CHANCE: f32 = 0.1;
const MAX_HEAD_Y_ROT: f32 = 15.0;

pub struct GoatEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub is_screaming: AtomicBool,
    pub has_left_horn: AtomicBool,
    pub has_right_horn: AtomicBool,
}

impl GoatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Goat.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            false,
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN, true);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN, true);
        let mut mob_entity = MobEntity::new(entity);
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_float(true);
        navigator.set_pathfinding_malus(PathType::PowderSnow, -1.0);
        // Vanilla's `PathType.ON_TOP_OF_POWDER_SNOW`.
        navigator.set_pathfinding_malus(PathType::DangerPowderSnow, -1.0);
        let goat = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            is_screaming: AtomicBool::new(false),
            has_left_horn: AtomicBool::new(true),
            has_right_horn: AtomicBool::new(true),
        };
        Arc::new(goat)
    }

    #[must_use]
    pub fn is_screaming(&self) -> bool {
        self.is_screaming.load(Ordering::Relaxed)
    }

    pub fn set_screaming(&self, screaming: bool) {
        self.is_screaming.store(screaming, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            screaming,
        );
    }

    #[must_use]
    pub fn has_left_horn(&self) -> bool {
        self.has_left_horn.load(Ordering::Relaxed)
    }

    pub fn set_has_left_horn(&self, has_horn: bool) {
        self.has_left_horn.store(has_horn, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN,
            has_horn,
        );
    }

    #[must_use]
    pub fn has_right_horn(&self) -> bool {
        self.has_right_horn.load(Ordering::Relaxed)
    }

    fn screaming_sound(&self, regular: Sound, screaming: Sound) -> Sound {
        if self.is_screaming() {
            screaming
        } else {
            regular
        }
    }

    pub fn set_has_right_horn(&self, has_horn: bool) {
        self.has_right_horn.store(has_horn, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN,
            has_horn,
        );
    }

    /// Vanilla `Goat.createHorn`: the instrument is picked with a random seeded by the UUID's
    /// hash, so every horn of one goat plays the same.
    #[must_use]
    pub fn create_horn(&self) -> ItemStack {
        let (most, least) = self.get_entity().entity_uuid.as_u64_pair();
        let hilo = most ^ least;
        let hash = ((hilo >> 32) as i32) ^ (hilo as i32);
        let mut random = LegacyRand::from_seed(i64::from(hash) as u64);
        let instruments = if self.is_screaming() {
            tag::Instrument::MINECRAFT_SCREAMING_GOAT_HORNS.0
        } else {
            tag::Instrument::MINECRAFT_REGULAR_GOAT_HORNS.0
        };
        let mut horn = ItemStack::new(1, &Item::GOAT_HORN);
        if !instruments.is_empty() {
            let index = random.next_bounded_i32(instruments.len() as i32) as usize;
            horn.set_data_component(InstrumentImpl {
                instrument: format!("minecraft:{}", instruments[index]).into(),
            });
        }
        horn
    }

    /// Vanilla `Goat.dropHorn`: an adult loses one of its horns, picked at random when it has
    /// both, and drops it as a goat horn item.
    pub fn drop_horn(&self) -> bool {
        if self.is_baby() {
            return false;
        }
        let (has_left, has_right) = (self.has_left_horn(), self.has_right_horn());
        if !has_left && !has_right {
            return false;
        }
        let random = &mut self.get_random();
        let drop_left = if !has_left {
            false
        } else if !has_right {
            true
        } else {
            random.random::<bool>()
        };
        if drop_left {
            self.set_has_left_horn(false);
        } else {
            self.set_has_right_horn(false);
        }
        let entity = self.get_entity();
        let horn = self.create_horn();
        let random_between =
            |min: f32, max: f32| f64::from(random.random::<f32>().mul_add(max - min, min));
        let velocity = Vector3::new(
            random_between(-0.2, 0.2),
            random_between(0.3, 0.7),
            random_between(-0.2, 0.2),
        );
        let world = entity.world.load_full();
        let item = Entity::new(world.clone(), entity.pos.load(), &EntityType::ITEM);
        world.spawn_entity(Arc::new(ItemEntity::new_with_velocity(
            item, horn, velocity, 0,
        )));
        true
    }
}

impl CustomSound for GoatEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(self.screaming_sound(Sound::EntityGoatDeath, Sound::EntityGoatScreamingDeath))
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.screaming_sound(Sound::EntityGoatHurt, Sound::EntityGoatScreamingHurt))
    }
}

impl AgeableMob for GoatEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn age_boundary_reached(&self) {
        let damage = if self.is_baby() {
            BABY_ATTACK_DAMAGE
        } else {
            ADULT_ATTACK_DAMAGE
        };
        self.mob_entity
            .living_entity
            .set_attribute_base(&Attributes::ATTACK_DAMAGE, damage);
    }
}

impl Animal for GoatEntity {
    fn play_eating_sound(&self) {
        let sound = if self.is_screaming() {
            Sound::EntityGoatScreamingEat
        } else {
            Sound::EntityGoatEat
        };
        let pitch = 0.8 + self.get_random().random::<f32>() * 0.4;
        self.play_entity_sound(sound, 1.0, pitch);
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_GOAT_FOOD)
    }

    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::GOAT,
        ));
        baby.mob_entity.with_brain(baby.as_ref(), |brain| {
            ai::init_memories(brain, baby.as_ref());
        });
        let random = self.get_random();
        let selected_parent_screaming = if random.random::<bool>() {
            self.is_screaming()
        } else {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(Self::is_screaming)
        };
        baby.set_screaming(
            selected_parent_screaming || random.random::<f64>() < GOAT_SCREAMING_CHANCE,
        );
        baby
    }
}

impl Mob for GoatEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.mob_entity.with_brain(self, ai::update_activity);
    }

    fn get_max_head_rotation(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.screaming_sound(Sound::EntityGoatAmbient, Sound::EntityGoatScreamingAmbient))
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity
            .with_brain(self, |brain| ai::init_memories(brain, self));
        let random = self.get_random();
        self.set_screaming(random.random::<f64>() < GOAT_SCREAMING_CHANCE);
        self.age_boundary_reached();
        if !self.is_baby() && random.random::<f32>() < UNIHORN_CHANCE {
            if random.random::<bool>() {
                self.set_has_left_horn(false);
            } else {
                self.set_has_right_horn(false);
            }
        }
        self.finalize_ageable_spawn(group_data)
    }

    fn calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        self.mob_entity
            .living_entity
            .base_fall_damage(fall_distance, damage_modifier)
            - GOAT_FALL_DAMAGE_REDUCTION
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityGoatStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsScreamingGoat", self.is_screaming());
        nbt.put_bool("HasLeftHorn", self.has_left_horn());
        nbt.put_bool("HasRightHorn", self.has_right_horn());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(screaming) = nbt.get_bool("IsScreamingGoat") {
            self.set_screaming(screaming);
        }
        if let Some(left) = nbt.get_bool("HasLeftHorn") {
            self.set_has_left_horn(left);
        }
        if let Some(right) = nbt.get_bool("HasRightHorn") {
            self.set_has_right_horn(right);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::goat::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            self.is_screaming(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN,
            self.has_left_horn(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN,
            self.has_right_horn(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.get_item() == &Item::BUCKET && !self.is_baby() {
            self.get_entity().world.load().play_sound_expect(
                player,
                self.screaming_sound(Sound::EntityGoatMilk, Sound::EntityGoatScreamingMilk),
                SoundCategory::Players,
                &player.position(),
            );
            fill_bucket_stack(item_stack, player, &Item::MILK_BUCKET);
            return true;
        }

        let consumed = self.animal_interact(player, item_stack);
        // Vanilla `Goat.mobInteract` plays the eating sound again on top of `Animal`'s, unless
        // the last food item was used up.
        if consumed && self.is_food(item_stack) {
            self.play_eating_sound();
        }
        consumed
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::math::vector3::Vector3;

    use super::*;
    use crate::entity::ai::test_support::world;

    #[tokio::test]
    async fn attack_damage_follows_the_age_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let goat = GoatEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.5, 64.0, 0.5),
            &EntityType::GOAT,
        ));
        let attack_damage = || {
            goat.mob_entity
                .living_entity
                .get_attribute_base(&Attributes::ATTACK_DAMAGE)
        };
        goat.set_baby(true);
        assert_eq!(attack_damage(), BABY_ATTACK_DAMAGE);
        goat.set_baby(false);
        assert_eq!(attack_damage(), ADULT_ATTACK_DAMAGE);
        world.level.shutdown().await;
        Ok(())
    }
}
