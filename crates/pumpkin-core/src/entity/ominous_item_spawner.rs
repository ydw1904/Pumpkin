use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI64, Ordering},
};

use pumpkin_data::{
    damage::DamageType,
    entity::EntityType,
    game_event::GameEvent,
    item::Item,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
    tracked_data,
    world::WorldEvent,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;

use crate::{
    block::blocks::redstone::dispenser::DispenserBlock,
    entity::{
        Entity, EntityBase,
        item::ItemEntity,
        living::LivingEntity,
        projectile::{
            ThrownItemEntity,
            arrow::{ArrowEntity, ArrowPickup},
            egg::EggEntity,
            experience_bottle::ExperienceBottleEntity,
            firework_rocket::FireworkRocketEntity,
            lingering_potion::LingeringPotionEntity,
            owner::ProjectileOwner,
            small_fireball::SmallFireballEntity,
            snowball::SnowballEntity,
            splash_potion::SplashPotionEntity,
            trident::TridentEntity,
            wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity},
        },
        projectile_deflection::LastDeflectedBy,
    },
    server::Server,
    world::{World, game_event::GameEventContext},
};

const SPAWN_ITEM_DELAY_MIN: i64 = 60;
const SPAWN_ITEM_DELAY_MAX: i64 = 120;
const TAG_SPAWN_ITEM_AFTER_TICKS: &str = "spawn_item_after_ticks";
const TAG_ITEM: &str = "item";
pub const TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND: i64 = 36;

/// Vanilla `OminousItemSpawner`, the item an ominous trial spawner drops on its players.
pub struct OminousItemSpawnerEntity {
    pub entity: Entity,
    item_stack: Mutex<ItemStack>,
    spawn_item_after_ticks: AtomicI64,
}

impl OminousItemSpawnerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        entity.synched_data.define(
            tracked_data::ominous_item_spawner::DATA_ITEM,
            ItemStackSerializer::from(ItemStack::EMPTY.clone()),
        );
        Arc::new(Self {
            entity,
            item_stack: Mutex::new(ItemStack::EMPTY.clone()),
            spawn_item_after_ticks: AtomicI64::new(0),
        })
    }

    /// Vanilla `OminousItemSpawner.create` followed by `snapTo(pos)`.
    #[must_use]
    pub fn create(world: Arc<World>, pos: Vector3<f64>, item: ItemStack) -> Arc<Self> {
        let spawner = Self::new(Entity::new(world, pos, &EntityType::OMINOUS_ITEM_SPAWNER));
        let delay = game_random().random_range(SPAWN_ITEM_DELAY_MIN..=SPAWN_ITEM_DELAY_MAX);
        spawner
            .spawn_item_after_ticks
            .store(delay, Ordering::Relaxed);
        spawner.set_item(item);
        spawner
    }

    #[must_use]
    pub fn get_item(&self) -> ItemStack {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_item(&self, item: ItemStack) {
        self.entity.set_synced_data(
            tracked_data::ominous_item_spawner::DATA_ITEM,
            ItemStackSerializer::from(item.clone()),
        );
        *self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item;
    }

    /// Vanilla `OminousItemSpawner.spawnItem`.
    fn spawn_item(&self) {
        let item = self.get_item();
        if item.is_empty() {
            return;
        }

        let world = self.entity.world.load();
        let pos = self.entity.pos.load();
        let spawned_entity = self
            .spawn_projectile(&world, pos, &item)
            .unwrap_or_else(|| {
                let item_entity = Arc::new(ItemEntity::new(
                    Entity::new(world.clone(), pos, &EntityType::ITEM),
                    item,
                ));
                world.spawn_entity(item_entity.clone());
                item_entity as Arc<dyn EntityBase>
            });

        world.sync_world_event(
            WorldEvent::ParticlesTrialSpawnerSpawnItem,
            self.entity.block_pos.load(),
            1,
        );
        world.emit_game_event_with_context(
            GameEvent::EntityPlace.name(),
            pos,
            &GameEventContext {
                source_entity: Some(spawned_entity.as_ref()),
                affected_state: None,
            },
        );
        self.set_item(ItemStack::EMPTY.clone());
    }

    /// Vanilla `OminousItemSpawner.spawnProjectile`: each `ProjectileItem.asProjectile`, shot
    /// straight down with the item's `DispenseConfig` and owned by the spawner. Returns `None`
    /// for items that aren't projectiles.
    #[expect(
        clippy::too_many_lines,
        reason = "one branch per vanilla `ProjectileItem`"
    )]
    fn spawn_projectile(
        &self,
        world: &Arc<World>,
        pos: Vector3<f64>,
        item: &ItemStack,
    ) -> Option<Arc<dyn EntityBase>> {
        let id = item.item.id;
        let block_pos = self.entity.block_pos.load();
        let new_entity = |entity_type| Entity::new(world.clone(), pos, entity_type);
        let aim = |thrown: &mut ThrownItemEntity, power, uncertainty| {
            thrown.owner = ProjectileOwner::new(Some(&self.entity));
            thrown.set_velocity(0.0, -1.0, 0.0, power, uncertainty);
        };
        // Vanilla `FireChargeItem.asProjectile` and `WindChargeItem.asProjectile` aim down with
        // their own spread, which the following `Projectile.shoot` replaces.
        let draw_charge_aim = || {
            for step in [0.0, -1.0, 0.0] {
                let _ =
                    game_random().next_triangular(step, DispenserBlock::CHARGE_DIRECTION_SPREAD);
            }
        };

        let projectile: Arc<dyn EntityBase> = if id == Item::ARROW.id
            || id == Item::TIPPED_ARROW.id
            || id == Item::SPECTRAL_ARROW.id
        {
            let mut arrow = ArrowEntity::new_with_item(
                new_entity(ArrowEntity::entity_type_for_item(item.item)),
                None,
                item,
                ArrowPickup::Allowed,
            );
            arrow.set_velocity(
                0.0,
                -1.0,
                0.0,
                DispenserBlock::DEFAULT_PROJECTILE_POWER,
                DispenserBlock::DEFAULT_PROJECTILE_UNCERTAINTY,
            );
            arrow.apply_on_projectile_spawned(item);
            arrow.owner = ProjectileOwner::new(Some(&self.entity));
            Arc::new(arrow)
        } else if id == Item::TRIDENT.id {
            let mut trident = TridentEntity::new_with_item(
                new_entity(&EntityType::TRIDENT),
                item.copy_with_count(1),
                ArrowPickup::Allowed,
            );
            trident.set_velocity(
                0.0,
                -1.0,
                0.0,
                DispenserBlock::DEFAULT_PROJECTILE_POWER,
                DispenserBlock::DEFAULT_PROJECTILE_UNCERTAINTY,
            );
            trident.apply_on_projectile_spawned(item);
            trident.owner = ProjectileOwner::new(Some(&self.entity));
            Arc::new(trident)
        } else if id == Item::SNOWBALL.id {
            let mut snowball = SnowballEntity::new(new_entity(&EntityType::SNOWBALL));
            aim(
                &mut snowball.thrown,
                DispenserBlock::DEFAULT_PROJECTILE_POWER,
                DispenserBlock::DEFAULT_PROJECTILE_UNCERTAINTY,
            );
            snowball.set_item_stack(item.copy_with_count(1));
            Arc::new(snowball)
        } else if id == Item::EGG.id || id == Item::BLUE_EGG.id || id == Item::BROWN_EGG.id {
            let mut egg = EggEntity::new(new_entity(&EntityType::EGG));
            aim(
                &mut egg.thrown,
                DispenserBlock::DEFAULT_PROJECTILE_POWER,
                DispenserBlock::DEFAULT_PROJECTILE_UNCERTAINTY,
            );
            egg.set_item_stack(item.clone());
            Arc::new(egg)
        } else if id == Item::SPLASH_POTION.id {
            let mut potion = SplashPotionEntity::new(new_entity(&EntityType::SPLASH_POTION));
            aim(
                &mut potion.thrown,
                DispenserBlock::POTION_PROJECTILE_POWER,
                DispenserBlock::POTION_PROJECTILE_UNCERTAINTY,
            );
            potion.set_item_stack(item.clone());
            Arc::new(potion)
        } else if id == Item::LINGERING_POTION.id {
            let mut potion = LingeringPotionEntity::new(new_entity(&EntityType::LINGERING_POTION));
            aim(
                &mut potion.thrown,
                DispenserBlock::POTION_PROJECTILE_POWER,
                DispenserBlock::POTION_PROJECTILE_UNCERTAINTY,
            );
            potion.set_item_stack(item.clone());
            Arc::new(potion)
        } else if id == Item::EXPERIENCE_BOTTLE.id {
            let mut bottle =
                ExperienceBottleEntity::new(new_entity(&EntityType::EXPERIENCE_BOTTLE));
            aim(
                &mut bottle.thrown,
                DispenserBlock::POTION_PROJECTILE_POWER,
                DispenserBlock::POTION_PROJECTILE_UNCERTAINTY,
            );
            bottle.set_item_stack(item.clone());
            Arc::new(bottle)
        } else if id == Item::FIREWORK_ROCKET.id {
            world.sync_world_event(WorldEvent::SoundFireworkShoot, block_pos, 0);
            let rocket = FireworkRocketEntity::new_with_owner(
                new_entity(&EntityType::FIREWORK_ROCKET),
                &self.entity,
                item.copy_with_count(1),
            );
            rocket.set_shot_at_angle(true);
            rocket.entity.set_velocity(
                0.0,
                -1.0,
                0.0,
                DispenserBlock::FIREWORK_PROJECTILE_POWER,
                DispenserBlock::FIREWORK_PROJECTILE_UNCERTAINTY,
            );
            Arc::new(rocket)
        } else if id == Item::FIRE_CHARGE.id {
            world.sync_world_event(WorldEvent::SoundBlazeFireball, block_pos, 0);
            let mut fireball = SmallFireballEntity::new(new_entity(&EntityType::SMALL_FIREBALL));
            draw_charge_aim();
            aim(
                &mut fireball.thrown,
                DispenserBlock::FIREBALL_PROJECTILE_POWER,
                DispenserBlock::FIREBALL_PROJECTILE_UNCERTAINTY,
            );
            fireball.item.set(fireball.get_entity(), item.clone());
            Arc::new(fireball)
        } else if id == Item::WIND_CHARGE.id {
            world.sync_world_event(WorldEvent::SoundWindChargeShoot, block_pos, 0);
            let mut thrown = ThrownItemEntity {
                entity: new_entity(&EntityType::WIND_CHARGE),
                owner: ProjectileOwner::none(),
                has_hit: AtomicBool::new(false),
                last_deflected_by: LastDeflectedBy::new(),
                gravity: WIND_CHARGE_GRAVITY,
            };
            draw_charge_aim();
            aim(
                &mut thrown,
                DispenserBlock::FIREBALL_PROJECTILE_POWER,
                DispenserBlock::FIREBALL_PROJECTILE_UNCERTAINTY,
            );
            Arc::new(WindChargeEntity::new_normal(thrown))
        } else {
            return None;
        };

        world.spawn_entity(projectile.clone());
        Some(projectile)
    }
}

impl EntityBase for OminousItemSpawnerEntity {
    fn init_data_tracker(&self) {
        let item = self.get_item();
        if !item.is_empty() {
            self.entity.set_synced_data(
                tracked_data::ominous_item_spawner::DATA_ITEM,
                ItemStackSerializer::from(item),
            );
        }
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        if self.entity.is_removed() {
            return;
        }

        // Vanilla `OminousItemSpawner.tickServer`.
        let tick_count = i64::from(self.entity.tick_count.load(Ordering::Relaxed));
        let spawn_after = self.spawn_item_after_ticks.load(Ordering::Relaxed);
        if tick_count == spawn_after - TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND {
            self.entity.world.load().play_sound(
                Sound::BlockTrialSpawnerAboutToSpawnItem,
                SoundCategory::Neutral,
                &self.entity.block_pos.load().to_centered_f64(),
            );
        }

        if tick_count >= spawn_after {
            self.spawn_item();
            self.kill(caller);
        }
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(item_nbt) = self.get_item().to_nbt() {
            nbt.put_compound(TAG_ITEM, item_nbt);
        }
        nbt.put_long(
            TAG_SPAWN_ITEM_AFTER_TICKS,
            self.spawn_item_after_ticks.load(Ordering::Relaxed),
        );
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let item = nbt
            .get_compound(TAG_ITEM)
            .and_then(ItemStack::read_item_stack)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        self.set_item(item);
        // Vanilla `getLongOr` takes any number, so `/summon` SNBT ints work too.
        let ticks = nbt
            .get_numeric_long(TAG_SPAWN_ITEM_AFTER_TICKS)
            .unwrap_or(0);
        self.spawn_item_after_ticks.store(ticks, Ordering::Relaxed);
    }

    fn is_ignoring_block_triggers(&self) -> bool {
        true
    }

    fn is_ignored_by_pistons(&self) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
