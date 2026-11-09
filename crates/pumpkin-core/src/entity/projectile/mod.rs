use super::{
    Entity, EntityBase,
    living::LivingEntity,
    projectile_deflection::{LastDeflectedBy, redirect_projectile},
};
use crate::world::World;
use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::{position::BlockPos, signum, vector3::Vector3};
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};
use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
pub mod arrow;
pub mod dragon_fireball;
pub mod egg;
pub mod ender_pearl;
pub mod evoker_fangs;
pub mod experience_bottle;
pub mod eye_of_ender;
pub mod fireball;
pub mod firework_rocket;
pub mod fishing_bobber;
pub mod lingering_potion;
pub mod llama_spit;
pub mod owner;
pub mod shulker_bullet;
pub mod small_fireball;
pub mod snowball;
pub mod splash_potion;
pub mod trident;
pub mod wind_charge;
pub mod wither_skull;

use pumpkin_data::item_stack::ItemStack;

/// The entities vanilla `ProjectileUtil.getEntityHitResult` considers: `level.getEntities`,
/// which includes the ender dragon's parts, without the dragon itself, which is not pickable.
pub(crate) fn hit_candidates(world: &World, area: &BoundingBox) -> Vec<Arc<dyn EntityBase>> {
    let mut candidates = world.get_all_at_box(area);
    candidates.retain(|entity| {
        !entity
            .cast_any()
            .is::<crate::entity::boss::ender_dragon::EnderDragonEntity>()
    });
    candidates.extend(world.get_dragon_parts_at_box(area));
    candidates
}

/// Vanilla `AbstractArrow`: arrows, spectral arrows and tridents.
#[must_use]
pub fn is_abstract_arrow(entity_type: &EntityType) -> bool {
    entity_type == &EntityType::ARROW
        || entity_type == &EntityType::SPECTRAL_ARROW
        || entity_type == &EntityType::TRIDENT
}

/// Vanilla `AbstractWindCharge`: wind charges and breeze wind charges.
fn is_wind_charge(entity_type: &EntityType) -> bool {
    entity_type == &EntityType::WIND_CHARGE || entity_type == &EntityType::BREEZE_WIND_CHARGE
}

#[must_use]
pub fn is_projectile(entity_type: &EntityType) -> bool {
    *entity_type == EntityType::ARROW
        || *entity_type == EntityType::TRIDENT
        || *entity_type == EntityType::EGG
        || *entity_type == EntityType::EXPERIENCE_BOTTLE
        || *entity_type == EntityType::SNOWBALL
        || *entity_type == EntityType::FIREWORK_ROCKET
        || *entity_type == EntityType::WIND_CHARGE
        || *entity_type == EntityType::SPLASH_POTION
        || *entity_type == EntityType::LINGERING_POTION
        || *entity_type == EntityType::ENDER_PEARL
        || *entity_type == EntityType::SHULKER_BULLET
        || *entity_type == EntityType::FIREBALL
        || *entity_type == EntityType::SMALL_FIREBALL
        || *entity_type == EntityType::FISHING_BOBBER
        || *entity_type == EntityType::WITHER_SKULL
        || *entity_type == EntityType::LLAMA_SPIT
}

/// Helper to apply projectile spawned enchantment effects matching vanilla `Projectile::applyOnProjectileSpawned`.
pub fn apply_on_projectile_spawned(
    projectile_entity: &Entity,
    pickup_item_stack: &ItemStack,
    weapon: Option<&ItemStack>,
    arrow: Option<&arrow::ArrowEntity>,
) {
    crate::enchantment::EnchantmentHelper::on_projectile_spawned(
        pickup_item_stack,
        projectile_entity,
        arrow,
    );
    if let Some(weapon) = weapon
        && weapon.item_count > 0
        && weapon.item.id != pickup_item_stack.item.id
    {
        crate::enchantment::EnchantmentHelper::on_projectile_spawned(
            weapon,
            projectile_entity,
            arrow,
        );
    }
}

/// Vanilla `AbstractHurtingProjectile.accelerationPower`: how much speed a hurting projectile
/// gains along its flight each tick.
pub struct AccelerationPower(AtomicU64);

impl AccelerationPower {
    #[must_use]
    pub const fn new(power: f64) -> Self {
        Self(AtomicU64::new(power.to_bits()))
    }

    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, power: f64) {
        self.0.store(power.to_bits(), Ordering::Relaxed);
    }

    /// Vanilla `AbstractHurtingProjectile.onDeflection`.
    pub fn on_deflection(&self, by_attack: bool) {
        if by_attack {
            self.set(fireball::INITIAL_ACCELERATION_POWER);
        } else {
            self.set(self.get() * fireball::DEFLECTION_SCALE);
        }
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_double("acceleration_power", self.get());
    }

    /// Vanilla `readAdditionalSaveData`: any number tag, and a missing one reads as the initial
    /// power, even for wind charges, which are constructed with none.
    pub fn read_nbt(&self, nbt: &NbtCompound) {
        self.set(
            nbt.get("acceleration_power")
                .and_then(NbtTag::as_numeric_double)
                .unwrap_or(fireball::INITIAL_ACCELERATION_POWER),
        );
    }
}

pub struct ThrownItemEntity {
    pub entity: Entity,
    pub owner: owner::ProjectileOwner,
    pub collides_with_projectiles: bool,
    pub has_hit: AtomicBool,
    pub last_deflected_by: LastDeflectedBy,
    pub gravity: f64,
}

impl ThrownItemEntity {
    pub fn new(entity: Entity, owner: &Entity, gravity: f64) -> Self {
        let mut owner_pos = owner.pos.load();
        owner_pos.y += owner.get_eye_height() - 0.1;
        entity.set_pos(owner_pos);
        Self {
            entity,
            owner: owner::ProjectileOwner::new(Some(owner)),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            last_deflected_by: LastDeflectedBy::new(),
            gravity,
        }
    }

    pub fn set_velocity_from(&self, pitch: f32, yaw: f32, roll: f32, speed: f32, divergence: f32) {
        let yaw_rad = yaw.to_radians();
        let pitch_rad = pitch.to_radians();
        let roll_rad = (pitch + roll).to_radians();

        let x = -yaw_rad.sin() * pitch_rad.cos();
        let y = -roll_rad.sin();
        let z = yaw_rad.cos() * pitch_rad.cos();

        self.set_velocity(
            f64::from(x),
            f64::from(y),
            f64::from(z),
            f64::from(speed),
            f64::from(divergence),
        );
    }

    pub fn set_velocity(&self, x: f64, y: f64, z: f64, power: f64, uncertainty: f64) {
        fn next_triangular(mode: f64, deviation: f64) -> f64 {
            deviation.mul_add(
                game_random().random::<f64>() - game_random().random::<f64>(),
                mode,
            )
        }
        let velocity = Vector3::new(x, y, z)
            .normalize()
            .add_raw(
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
            )
            .multiply(power, power, power);

        self.entity.velocity.store(velocity);
        let len = velocity.horizontal_length();
        self.entity.set_rotation(
            velocity.x.atan2(velocity.z) as f32 * 57.295_776,
            velocity.y.atan2(len) as f32 * 57.295_776,
        );
    }
}

impl ThrownItemEntity {
    /// Process a tick for projectile movement and collisions
    pub fn process_tick(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        self.owner.check_left_owner(entity);
        entity.update_last_pos();

        // Apply gravity and inertia
        let mut velocity = entity.velocity.load();
        velocity.y -= self.get_gravity();

        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            0.8
        } else {
            0.99
        };
        velocity = velocity.multiply(inertia, inertia, inertia);

        // Store velocity
        entity.velocity.store(velocity);

        self.move_and_hit(caller);
    }

    /// Vanilla `AbstractHurtingProjectile.tick`: no gravity or drag, but `acceleration_power`
    /// along the flight and `inertia` (`liquid_inertia` in water) instead. Leaving the loaded
    /// chunks discards the projectile. `should_burn` keeps it alight, like vanilla `shouldBurn`.
    pub fn process_hurting_tick(
        &self,
        caller: &dyn EntityBase,
        acceleration_power: f64,
        inertia: f32,
        liquid_inertia: f32,
        should_burn: bool,
    ) {
        let entity = self.get_entity();
        let movement = entity.velocity.load();
        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            liquid_inertia
        } else {
            inertia
        };
        // `Vec3.normalize` is zero for a vector shorter than 1.0E-5.
        let length = movement.length();
        let direction = if length < 1.0E-5 {
            Vector3::default()
        } else {
            movement * (1.0 / length)
        };
        entity
            .velocity
            .store((movement + direction * acceleration_power) * f64::from(inertia));
        if !entity.world.load().is_loaded(&entity.block_pos.load()) {
            entity.remove();
            return;
        }
        self.owner.check_left_owner(entity);
        entity.update_last_pos();
        rotate_towards_movement(entity, HURTING_ROTATION_SPEED);
        Self::tick_hurting_fire(entity, should_burn);
        self.move_and_hit(caller);
    }

    /// The fire part of vanilla `Entity.baseTick` (a projectile takes no fire damage), then
    /// `igniteForSeconds(1.0F)` when the projectile burns. Paper doesn't fire a combust event
    /// for this, so neither does Pumpkin.
    fn tick_hurting_fire(entity: &Entity, should_burn: bool) {
        let fire_ticks = entity.fire_ticks.load(Ordering::Relaxed);
        if fire_ticks > 0 {
            if entity.is_fire_immune() {
                entity.extinguish();
            } else {
                entity.fire_ticks.store(fire_ticks - 1, Ordering::Relaxed);
            }
        }
        entity.set_on_fire(entity.fire_ticks.load(Ordering::Relaxed) > 0);
        if should_burn && !entity.is_fire_immune() {
            entity
                .fire_ticks
                .fetch_max(HURTING_BURN_TICKS, Ordering::Relaxed);
        }
    }

    /// Moves the projectile by its velocity, then hits the first block or entity on the way.
    fn move_and_hit(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let velocity = entity.velocity.load();
        let start_pos = entity.pos.load();
        let delta = velocity;

        let new_pos = start_pos.add(&delta);

        // Calculate search box for collisions
        let search_box = BoundingBox::new(
            Vector3::new(
                start_pos.x.min(new_pos.x),
                start_pos.y.min(new_pos.y),
                start_pos.z.min(new_pos.z),
            ),
            Vector3::new(
                start_pos.x.max(new_pos.x),
                start_pos.y.max(new_pos.y),
                start_pos.z.max(new_pos.z),
            ),
        )
        .expand(0.3, 0.3, 0.3);

        let mut closest_t = 1.0f64;
        let mut hit = None;

        // Block collisions. Vanilla `BlockGetter.clip` misses when the ray has no length, so a
        // projectile at rest on a block doesn't hit it.
        let (block_cols, block_positions) = if delta.length_squared() > 0.0 {
            world.get_block_collisions(search_box, caller)
        } else {
            Default::default()
        };
        for (idx, bb) in block_cols.iter().enumerate() {
            if let Some(t) = calculate_ray_intersection(&start_pos, &delta, bb)
                && t < closest_t
            {
                closest_t = t;
                // Map back to block pos
                for (len, pos) in &block_positions {
                    if idx < *len {
                        let hit_pos = start_pos.add(&delta.multiply(t, t, t));
                        hit = Some(ProjectileHit::Block {
                            pos: *pos,
                            face: get_hit_face(hit_pos, *pos),
                            hit_pos,
                            normal: delta.normalize().multiply(-1.0, -1.0, -1.0),
                        });
                        break;
                    }
                }
            }
        }

        // Entities only count up to the block hit, like vanilla `ProjectileUtil.getHitResult`.
        let to = start_pos + delta * closest_t;
        let candidates = hit_candidates(&world, &search_box)
            .into_iter()
            .filter(|cand| !self.should_skip_collision(&world, entity, cand));
        if let Some((target, hit_pos)) =
            entity_hit_result(candidates, start_pos, to, compute_margin(entity))
        {
            hit = Some(ProjectileHit::Entity {
                entity: target,
                hit_pos,
                normal: delta.normalize().multiply(-1.0, -1.0, -1.0),
            });
        }

        // Vanilla moves the projectile to where it hits, so the hit (an explosion, a knockback
        // source) happens there.
        entity.set_pos(hit.as_ref().map_or(new_pos, ProjectileHit::hit_pos));
        // Vanilla `baseTick` after the move: the water state the next tick's inertia reads.
        entity.update_fluid_state(caller);

        // Handle hit or continue
        if let Some(h) = hit {
            if let ProjectileHit::Entity { entity: target, .. } = &h
                && self
                    .last_deflected_by
                    .hit_target_or_deflect_self(caller, target.as_ref())
            {
                return;
            }
            // Ensure hit is only processed once per projectile
            if self.has_hit.swap(true, Ordering::SeqCst) {
                return;
            }

            if let ProjectileHit::Block { pos, hit_pos, .. } = &h {
                let block = world.get_block(pos);
                let state = world.get_block_state(pos);
                if let Some(server) = world.server.upgrade() {
                    world
                        .block_registry
                        .on_projectile_hit(block, &world, caller, pos, state, hit_pos, &server);
                }
            }

            if let ProjectileHit::Entity { entity: target, .. } = &h {
                redirect_projectile(caller, target.as_ref());
            }
            // Just trigger hit effects and remove
            caller.on_hit(h);
            entity.remove();
        }
    }

    /// Returns if collision should be skipped (e.g. owner or projectile vs projectile)
    fn should_skip_collision(
        &self,
        world: &World,
        self_ent: &Entity,
        other: &Arc<dyn EntityBase>,
    ) -> bool {
        let other_ent = other.get_entity();
        if other_ent.entity_id == self_ent.entity_id || other.is_spectator() {
            return true;
        }

        if !self.owner.can_hit(world, other_ent) {
            return true;
        }

        // Not pickable in vanilla: projectiles pass through these
        // TODO: replace with `!can_hit()` once Player, TNT, falling blocks, end crystals, item
        // frames, shulker bullets and redirectable projectiles override it like vanilla `isPickable`
        if *other_ent.entity_type == EntityType::AREA_EFFECT_CLOUD
            || *other_ent.entity_type == EntityType::EXPERIENCE_ORB
            || *other_ent.entity_type == EntityType::ITEM
        {
            return true;
        }

        // Projectile vs projectile logic: vanilla only picks redirectable projectiles.
        if !self.collides_with_projectiles
            && is_projectile(other_ent.entity_type)
            && !other.can_hit()
        {
            return true;
        }

        // Vanilla `AbstractWindCharge.canHitEntity`: wind charges pass through each other and
        // through end crystals.
        if is_wind_charge(self_ent.entity_type)
            && (is_wind_charge(other_ent.entity_type)
                || *other_ent.entity_type == EntityType::END_CRYSTAL)
        {
            return true;
        }

        false
    }

    const fn get_entity(&self) -> &Entity {
        &self.entity
    }

    #[allow(dead_code, clippy::unused_self)]
    const fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    const fn get_gravity(&self) -> f64 {
        self.gravity
    }
}

/// Vanilla `AbstractHurtingProjectile.getInertia`, kept per tick outside fluids.
pub const HURTING_INERTIA: f32 = 0.95;
/// Vanilla `AbstractHurtingProjectile.getLiquidInertia`.
pub const HURTING_LIQUID_INERTIA: f32 = 0.8;

/// The rotation speed `AbstractHurtingProjectile.tick` turns with.
const HURTING_ROTATION_SPEED: f32 = 0.2;
/// Vanilla `igniteForSeconds(1.0F)` in `AbstractHurtingProjectile.tick`.
const HURTING_BURN_TICKS: i32 = 20;

/// Vanilla `ProjectileUtil.rotateTowardsMovement`: turns `speed` of the way from the current
/// rotation towards the flight direction.
// Vanilla's `Mth.lerp` rounds after the multiply and after the add, so no `mul_add` here.
#[allow(clippy::suboptimal_flops)]
pub(crate) fn rotate_towards_movement(entity: &Entity, speed: f32) {
    let movement = entity.velocity.load();
    if movement.length_squared() == 0.0 {
        return;
    }
    let horizontal = movement.horizontal_length();
    let to_degrees = 180.0 / f64::from(std::f32::consts::PI);
    let yaw = (movement.z.atan2(movement.x) * to_degrees) as f32 + 90.0;
    let pitch = (horizontal.atan2(movement.y) * to_degrees) as f32 - 90.0;
    let mut old_yaw = entity.yaw.load();
    let mut old_pitch = entity.pitch.load();
    while pitch - old_pitch < -180.0 {
        old_pitch -= 360.0;
    }
    while pitch - old_pitch >= 180.0 {
        old_pitch += 360.0;
    }
    while yaw - old_yaw < -180.0 {
        old_yaw -= 360.0;
    }
    while yaw - old_yaw >= 180.0 {
        old_yaw += 360.0;
    }
    entity.yaw.store(old_yaw + speed * (yaw - old_yaw));
    entity.pitch.store(old_pitch + speed * (pitch - old_pitch));
}

/// How far vanilla `AbstractArrow.onHitBlock` backs an arrow off along each axis it moved on.
const ARROW_BLOCK_HIT_OFFSET: f32 = 0.05;

/// Vanilla `AbstractArrow.onHitBlock`: where an arrow or trident that hit a block at `hit_pos`
/// while moving by `movement` comes to rest.
pub(crate) fn arrow_rest_position(hit_pos: Vector3<f64>, movement: Vector3<f64>) -> Vector3<f64> {
    let direction = Vector3::new(signum(movement.x), signum(movement.y), signum(movement.z));
    hit_pos - direction * f64::from(ARROW_BLOCK_HIT_OFFSET)
}

/// Vanilla `ProjectileUtil.DEFAULT_ENTITY_HIT_RESULT_MARGIN`.
const DEFAULT_ENTITY_HIT_RESULT_MARGIN: f32 = 0.3;

/// Vanilla `ProjectileUtil.computeMargin`: a projectile hits entities only by their exact box for
/// its first two ticks, then grows the margin around them to 0.3 over the next six.
pub(crate) fn compute_margin(entity: &Entity) -> f64 {
    let ticks = entity.tick_count.load(Ordering::Relaxed);
    f64::from(((ticks - 2) as f32 / 20.0).clamp(0.0, DEFAULT_ENTITY_HIT_RESULT_MARGIN))
}

/// Vanilla `ProjectileUtil.getEntityHitResult` with a margin: the candidate whose box, grown by
/// `margin`, the segment from `from` to `to` enters first. A segment starting inside a box misses
/// that entity.
pub(crate) fn entity_hit_result(
    candidates: impl IntoIterator<Item = Arc<dyn EntityBase>>,
    from: Vector3<f64>,
    to: Vector3<f64>,
    margin: f64,
) -> Option<(Arc<dyn EntityBase>, Vector3<f64>)> {
    let mut nearest = f64::MAX;
    let mut hit = None;
    for candidate in candidates {
        let bounding_box = candidate.get_entity().bounding_box.load();
        if let Some(location) = bounding_box.expand_all(margin).clip(from, to) {
            let distance = from.squared_distance_to_vec(&location);
            if distance < nearest {
                nearest = distance;
                hit = Some((candidate, location));
            }
        }
    }
    hit
}

/// Vanilla `ProjectileUtil.getManyEntityHitResult` as `AbstractArrow.findHitEntities` calls it:
/// every candidate the segment enters, at its exact box, or else where it enters the box grown by
/// `margin` when no block stands between there and the entity.
pub(crate) fn many_entity_hit_results(
    world: &World,
    candidates: impl IntoIterator<Item = Arc<dyn EntityBase>>,
    from: Vector3<f64>,
    to: Vector3<f64>,
    margin: f64,
) -> Vec<(Arc<dyn EntityBase>, Vector3<f64>)> {
    let mut hits = Vec::new();
    for candidate in candidates {
        let bounding_box = candidate.get_entity().bounding_box.load();
        if let Some(location) = bounding_box.clip(from, to) {
            hits.push((candidate, location));
        } else if margin > 0.0
            && let Some(outside) = bounding_box.expand_all(margin).clip(from, to)
        {
            let block_hit = world.clip(&ClipContext::new(
                outside,
                bounding_box.get_center(),
                ClipBlock::Collider,
                ClipFluid::None,
            ));
            let towards = if block_hit.is_miss {
                bounding_box.get_center()
            } else {
                block_hit.location
            };
            if bounding_box.clip(outside, towards).is_some() {
                hits.push((candidate, outside));
            }
        }
    }
    hits
}

/// Ray intersection algorithm for AABBs, returning a t value
pub(crate) fn calculate_ray_intersection(
    start: &Vector3<f64>,
    dir: &Vector3<f64>,
    bb: &BoundingBox,
) -> Option<f64> {
    let mut t_min = 0.0f64;
    let mut t_max = 1.0f64;

    let b_min = [bb.min.x, bb.min.y, bb.min.z];
    let b_max = [bb.max.x, bb.max.y, bb.max.z];
    let s = [start.x, start.y, start.z];
    let d = [dir.x, dir.y, dir.z];

    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if s[i] < b_min[i] || s[i] > b_max[i] {
                return None;
            }
        } else {
            let t1 = (b_min[i] - s[i]) / d[i];
            let t2 = (b_max[i] - s[i]) / d[i];
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
        }
    }

    (t_min <= t_max && (0.0..=1.0).contains(&t_min)).then_some(t_min)
}

/// Get the face of the block that was hit
fn get_hit_face(hit_pos: Vector3<f64>, block_pos: BlockPos) -> BlockDirection {
    let local = hit_pos.sub(&block_pos.0.to_f64());
    let eps = 1.0e-4;

    if local.x <= eps {
        BlockDirection::West
    } else if local.x >= 1.0 - eps {
        BlockDirection::East
    } else if local.y <= eps {
        BlockDirection::Down
    } else if local.y >= 1.0 - eps {
        BlockDirection::Up
    } else if local.z <= eps {
        BlockDirection::North
    } else {
        BlockDirection::South
    }
}

pub enum ProjectileHit {
    Block {
        pos: BlockPos,
        face: BlockDirection,
        hit_pos: Vector3<f64>,
        normal: Vector3<f64>,
    },
    Entity {
        entity: Arc<dyn EntityBase>,
        hit_pos: Vector3<f64>,
        normal: Vector3<f64>,
    },
}

impl ProjectileHit {
    /// Returns the exact impact coordinates regardless of what was hit.
    #[must_use]
    pub const fn hit_pos(&self) -> Vector3<f64> {
        match self {
            Self::Block { hit_pos, .. } | Self::Entity { hit_pos, .. } => *hit_pos,
        }
    }

    /// Returns the surface normal of the impact.
    #[must_use]
    pub const fn normal(&self) -> Vector3<f64> {
        match self {
            Self::Block { normal, .. } | Self::Entity { normal, .. } => *normal,
        }
    }

    /// Safely returns the face hit if it was a block, otherwise None.
    #[must_use]
    pub const fn face(&self) -> Option<BlockDirection> {
        match self {
            Self::Block { face, .. } => Some(*face),
            Self::Entity { .. } => None,
        }
    }
}
