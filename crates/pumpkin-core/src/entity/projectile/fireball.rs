use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::atomic_f32::AtomicF32;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase,
        projectile::{
            AccelerationPower, HURTING_INERTIA, HURTING_LIQUID_INERTIA, ProjectileHit,
            ThrownItemEntity,
        },
    },
    server::Server,
};

pub const MIN_CAMERA_DISTANCE_SQUARED: f32 = 12.25;
pub const INITIAL_ACCELERATION_POWER: f64 = 0.1;
pub const DEFLECTION_SCALE: f64 = 0.5;
pub const DEFAULT_EXPLOSION_POWER: f32 = 1.0;

/// The damage type of vanilla `DamageSources.fireball`, which loses its attribution without an
/// owner.
#[must_use]
pub fn fireball_damage_type(owner: Option<&dyn EntityBase>) -> DamageType {
    if owner.is_some() {
        DamageType::FIREBALL
    } else {
        DamageType::UNATTRIBUTED_FIREBALL
    }
}

/// Vanilla `Fireball`'s item, which large and small fireballs render as and save as `Item`.
pub struct FireballItem(RwLock<ItemStack>);

impl FireballItem {
    #[must_use]
    pub fn new() -> Self {
        Self(RwLock::new(Self::default_item()))
    }

    #[must_use]
    pub fn default_item() -> ItemStack {
        ItemStack::new(1, &Item::FIRE_CHARGE)
    }

    pub fn get(&self) -> ItemStack {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `Fireball.setItem`: one of `source`, or a fire charge for an empty stack.
    pub fn set(&self, entity: &Entity, source: ItemStack) {
        let item = if source.is_empty() {
            Self::default_item()
        } else {
            let mut item = source;
            item.item_count = 1;
            item
        };
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item;
        self.init_data_tracker(entity);
    }

    /// Vanilla compares item stacks by identity, so the item is never at its default and always
    /// goes out with the spawn packet.
    pub fn init_data_tracker(&self, entity: &Entity) {
        entity.set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(self.get()),
        );
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        let mut item = NbtCompound::new();
        self.get().write_item_stack(&mut item);
        nbt.put_compound("Item", item);
    }

    /// Vanilla `Fireball.readAdditionalSaveData`: a missing or invalid `Item` is a fire charge.
    pub fn read_nbt(&self, entity: &Entity, nbt: &NbtCompound) {
        let item = nbt
            .get_compound("Item")
            .and_then(ItemStack::read_item_stack)
            .unwrap_or_else(Self::default_item);
        self.set(entity, item);
    }
}

impl Default for FireballItem {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FireballEntity {
    pub thrown: ThrownItemEntity,
    pub item: FireballItem,
    pub acceleration_power: AccelerationPower,
    pub explosion_power: AtomicF32,
}

impl FireballEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            last_deflected_by: crate::entity::projectile_deflection::LastDeflectedBy::new(),
            gravity: 0.0,
        };

        Self {
            thrown,
            item: FireballItem::new(),
            acceleration_power: AccelerationPower::new(INITIAL_ACCELERATION_POWER),
            explosion_power: AtomicF32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, 0.0);
        let accel = INITIAL_ACCELERATION_POWER;
        let vel = direction.normalize().multiply(accel, accel, accel);
        thrown.entity.velocity.store(vel);

        Self {
            thrown,
            item: FireballItem::new(),
            acceleration_power: AccelerationPower::new(accel),
            explosion_power: AtomicF32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn new_directional(
        entity: Entity,
        direction: Vector3<f64>,
        acceleration_power: f64,
    ) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner: ProjectileOwner::none(),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            last_deflected_by: crate::entity::projectile_deflection::LastDeflectedBy::new(),
            gravity: 0.0,
        };
        let vel = direction.normalize().multiply(
            acceleration_power,
            acceleration_power,
            acceleration_power,
        );
        thrown.entity.velocity.store(vel);

        Self {
            thrown,
            item: FireballItem::new(),
            acceleration_power: AccelerationPower::new(acceleration_power),
            explosion_power: AtomicF32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    pub fn get_explosion_power(&self) -> f32 {
        self.explosion_power.load(Ordering::Relaxed)
    }

    pub fn set_explosion_power(&self, power: f32) {
        self.explosion_power.store(power, Ordering::Relaxed);
    }

    pub fn should_render_at_sqr_distance(&self, distance_sqr: f64) -> bool {
        if self.get_entity().tick_count.load(Ordering::Relaxed) < 2
            && distance_sqr < f64::from(MIN_CAMERA_DISTANCE_SQUARED)
        {
            false
        } else {
            let bb_size = self
                .get_entity()
                .bounding_box
                .load()
                .get_average_side_length()
                * 4.0;
            let size = if bb_size.is_nan() { 4.0 } else { bb_size } * 64.0;
            distance_sqr < size * size
        }
    }
}

impl EntityBase for FireballEntity {
    /// Vanilla `Projectile.isPickable`: redirectable projectiles can be hit.
    fn can_hit(&self) -> bool {
        true
    }

    /// Vanilla `AbstractHurtingProjectile.onDeflection`.
    fn on_deflection(&self, by_attack: bool) {
        self.acceleration_power.on_deflection(by_attack);
    }

    fn acceleration_power(&self) -> Option<f64> {
        Some(self.acceleration_power.get())
    }

    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.thrown.owner)
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.acceleration_power.write_nbt(nbt);
        self.item.write_nbt(nbt);
        // Vanilla `LargeFireball` keeps an int power and saves it as a byte.
        nbt.put_byte("ExplosionPower", self.get_explosion_power() as i8);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.acceleration_power.read_nbt(nbt);
        self.item.read_nbt(self.get_entity(), nbt);
        // Vanilla `getByteOr` takes any number tag, so `{ExplosionPower:4}` works.
        if let Some(power) = nbt.get("ExplosionPower").and_then(NbtTag::as_numeric_int) {
            self.set_explosion_power(f32::from(power as i8));
        }
    }

    fn init_data_tracker(&self) {
        self.item.init_data_tracker(self.get_entity());
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_hurting_tick(
            caller,
            self.acceleration_power.get(),
            HURTING_INERTIA,
            HURTING_LIQUID_INERTIA,
            true,
        );
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let world = self.get_entity().world.load();

        if let ProjectileHit::Entity { ref entity, .. } = hit {
            let owner = self.thrown.owner.get(&world);
            let damage_type = fireball_damage_type(owner.as_deref());
            if entity.damage_with_context(
                entity.as_ref(),
                6.0,
                damage_type,
                None,
                Some(self),
                owner.as_deref(),
            ) {
                crate::enchantment::EnchantmentHelper::do_post_attack_effects(
                    entity.as_ref(),
                    damage_type,
                    owner.as_deref(),
                    Some(self),
                );
            }
        }

        let hit_pos = hit.hit_pos();
        let power = self.get_explosion_power();
        // Vanilla `LargeFireball.onHit` sets fire when `mob_griefing` is on.
        if world.level_info.load().game_rules.mob_griefing {
            world.explode_with_fire(
                Some(self),
                hit_pos,
                power,
                crate::world::ExplosionInteraction::Mob,
            );
        } else {
            world.explode(
                Some(self),
                hit_pos,
                power,
                crate::world::ExplosionInteraction::Mob,
            );
        }
    }
}
