//! Vanilla `Mob.finalizeSpawn`: the step run on freshly created mobs before they enter the world.

use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity_variant::{HorseVariant, RabbitVariant};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use time::{Month, OffsetDateTime};

use crate::entity::EntityBase;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::mob::MobEntity;
use crate::entity::mob::equipment::{EQUIPMENT_REGISTRY, RegionalDifficulty, equip_mob_on_spawn};
use crate::entity::passive::fox::FoxVariant;
use crate::world::World;

pub(crate) const RANDOM_SPAWN_BONUS_ID: &str = "minecraft:random_spawn_bonus";

/// State shared by every mob of one spawn group (vanilla `SpawnGroupData`).
pub enum SpawnGroupData {
    Ageable {
        group_size: u32,
        should_spawn_baby: bool,
        baby_spawn_chance: f32,
    },
    School(std::sync::Weak<dyn EntityBase>),
    TropicalFish {
        leader: std::sync::Weak<dyn EntityBase>,
        variant: i32,
    },
    /// The effect the first spider of a group rolled on hard difficulty.
    SpiderEffects(Option<&'static StatusEffect>),
    /// A llama spawn group sharing the first llama's variant.
    Llama {
        variant: pumpkin_data::entity_variant::LlamaVariant,
        group_size: u32,
    },
    /// A horse spawn group sharing the first horse's coat color.
    Horse {
        color: HorseVariant,
        group_size: u32,
    },
    /// Vanilla `FoxGroupData`: the group's variant and how many joined so far.
    Fox {
        variant: FoxVariant,
        size: u32,
    },
    Rabbit {
        variant: RabbitVariant,
        group_size: u32,
    },
    /// Vanilla `AxolotlGroupData`: the two variants the group picks from.
    Axolotl {
        variants: [crate::entity::passive::axolotl::AxolotlVariant; 2],
        group_size: u32,
    },
    /// Vanilla `WolfPackData`: the variant the whole pack shares. It never spawns babies.
    WolfPack(pumpkin_data::wolf_variant::WolfVariant),
    /// Vanilla `ZombieGroupData`: whether the group spawns as babies, and whether those may
    /// ride a chicken.
    Zombie {
        is_baby: bool,
        can_spawn_jockey: bool,
    },
}

/// A vehicle a mob being finalized starts riding once it enters the world.
pub enum PendingVehicle {
    /// Already in the world.
    Existing(Arc<dyn EntityBase>),
    /// Created for the mob; added to the world together with it.
    New(Arc<dyn EntityBase>),
}

impl MobEntity {
    /// Vanilla `Mob.finalizeSpawn` followed by the loot pickup and equipment rolls of the
    /// subclasses.
    pub fn finalize_spawn_base(&self) {
        self.finalize_conversion_base();
        self.populate_spawn_equipment();
    }

    /// Vanilla `Mob.finalizeSpawn` alone: random follow range bonus and left-handedness. A
    /// conversion (`EntitySpawnReason.CONVERSION`) skips the loot pickup and equipment rolls.
    pub fn finalize_conversion_base(&self) {
        let rng = game_random();
        {
            let mut attributes = self
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(follow_range) = attributes.get_mut(&Attributes::FOLLOW_RANGE.id)
                && !follow_range
                    .modifiers
                    .iter()
                    .any(|modifier| modifier.id == RANDOM_SPAWN_BONUS_ID)
            {
                follow_range.add_or_replace_permanent_modifier(Modifier {
                    id: RANDOM_SPAWN_BONUS_ID.to_string(),
                    // Triangle distribution around 0.
                    amount: 0.114_85 * f64::from(rng.random::<f32>() - rng.random::<f32>()),
                    operation: ModifierOperation::MultiplyBase,
                });
            }
        }
        self.set_left_handed(rng.random::<f32>() < 0.05);
    }

    /// The Halloween part of vanilla `Zombie.finalizeSpawn` and `AbstractSkeleton.finalizeSpawn`:
    /// a bare head sometimes gets a carved pumpkin or jack o'lantern that never drops.
    pub fn wear_halloween_head(&self) {
        let head_empty = self
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::HEAD)
            .is_empty();
        if !head_empty || !is_halloween() || game_random().random::<f32>() >= 0.25 {
            return;
        }
        let head = if game_random().random::<f32>() < 0.1 {
            &Item::JACK_O_LANTERN
        } else {
            &Item::CARVED_PUMPKIN
        };
        self.set_item_slot(&EquipmentSlot::HEAD, ItemStack::new(1, head));
        self.living_entity
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(EquipmentSlot::HEAD, 0.0);
    }

    /// Loot pickup and spawn equipment of the mobs in `EQUIPMENT_REGISTRY`, rolled once in
    /// `finalizeSpawn` like vanilla (`setCanPickUpLoot`, `populateDefaultEquipmentSlots`), never
    /// when a saved mob is loaded.
    fn populate_spawn_equipment(&self) {
        let entity = &self.living_entity.entity;
        let Some(def) = EQUIPMENT_REGISTRY.get(entity.entity_type.resource_name) else {
            return;
        };
        let world = entity.world.load();
        if def.can_pick_up_loot {
            let difficulty = RegionalDifficulty::at(&world, entity.pos.load());
            self.set_can_pick_up_loot(
                game_random().random::<f32>() < 0.55 * difficulty.special_multiplier,
            );
        }
        equip_mob_on_spawn(self, &world);
    }

    /// Queues a mob to be added and mounted onto this one when it enters the world.
    pub fn add_pending_rider(&self, rider: Arc<dyn EntityBase>) {
        self.pending_riders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(rider);
    }

    /// Rides `vehicle` once this mob enters the world (vanilla `startRiding` in `finalizeSpawn`).
    pub fn set_pending_vehicle(&self, vehicle: PendingVehicle) {
        *self
            .pending_vehicle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(vehicle);
    }

    pub(crate) fn take_pending_vehicle(&self) -> Option<PendingVehicle> {
        self.pending_vehicle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) fn take_pending_riders(&self) -> Vec<Arc<dyn EntityBase>> {
        std::mem::take(
            &mut *self
                .pending_riders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

/// Vanilla `SpecialDates.isHalloween`: October 31st in the server's local time.
fn is_halloween() -> bool {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    now.month() == Month::October && now.day() == 31
}

/// Why a mob is being spawned, passed to [`finalize_spawn`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntitySpawnReason {
    Natural,
    ChunkGeneration,
    Spawner,
    Structure,
    Breeding,
    MobSummoned,
    Jockey,
    Event,
    Conversion,
    Reinforcement,
    Triggered,
    Bucket,
    SpawnItemUse,
    Command,
    Dispenser,
    Patrol,
    TrialSpawner,
    Load,
    DimensionTravel,
}

impl EntitySpawnReason {
    #[must_use]
    pub const fn is_spawner(self) -> bool {
        matches!(self, Self::Spawner | Self::TrialSpawner)
    }

    #[must_use]
    pub const fn ignores_light_requirements(self) -> bool {
        matches!(self, Self::TrialSpawner)
    }
}

/// Finalizes `entity` if it is a mob, and returns the group data for the next mob of the group.
pub fn finalize_spawn(
    entity: &Arc<dyn EntityBase>,
    world: &Arc<World>,
    reason: EntitySpawnReason,
    group_data: Option<SpawnGroupData>,
) -> Option<SpawnGroupData> {
    match entity.get_mob() {
        Some(mob) => mob.finalize_spawn(world, reason, group_data),
        None => group_data,
    }
}

impl MobEntity {
    /// Baby state kept only as a negative age and the shared baby flag (zombified piglins,
    /// which never grow up).
    pub fn set_baby_by_age(&self) {
        let entity = &self.living_entity.entity;
        entity
            .age
            .store(-24000, std::sync::atomic::Ordering::Relaxed);
        entity.set_synced_data(pumpkin_data::tracked_data::ageable_mob::DATA_BABY_ID, true);
        entity.refresh_dimensions();
    }

    /// Baby state kept as a mob's own flag and synced key (vanilla zombies and piglins), so it
    /// never ages up.
    pub fn set_baby_flag(
        &self,
        flag: &std::sync::atomic::AtomicBool,
        tracked: pumpkin_data::tracked_data::TrackedData,
        baby: bool,
    ) {
        flag.store(baby, std::sync::atomic::Ordering::Relaxed);
        let entity = &self.living_entity.entity;
        entity.set_synced_data(tracked, baby);
        // Pumpkin's negative age is what reports the Bedrock baby flag.
        entity.age.store(
            if baby { -24000 } else { 0 },
            std::sync::atomic::Ordering::Relaxed,
        );
        entity.refresh_dimensions();
    }
}
