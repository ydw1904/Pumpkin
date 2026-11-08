use pumpkin_util::random::gameplay::game_random;
use rustc_hash::FxHashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockDirection;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType::MotionBlocking;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::{Control, MoveControlTrait, move_control::Operation};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::projectile::fireball::FireballEntity;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{Controls, Goal},
    mob::{Mob, MobEntity},
};
use crate::world::World;

/// Vanilla `Ghast.leashElasticDistance`, shared with the happy ghast.
pub const LEASH_ELASTIC_DISTANCE: f64 = 10.0;
/// Vanilla `Ghast.leashSnapDistance`, shared with the happy ghast.
pub const LEASH_SNAP_DISTANCE: f64 = 16.0;
/// What vanilla `Ghast.hurtServer` deals for a fireball a player sent back.
const REFLECTED_FIREBALL_DAMAGE: f32 = 1000.0;

const GHAST_HUNTS: &[Hunt] = &[Hunt::new(1, TargetMatch::Type(&EntityType::PLAYER), true)
    .matching(hunting::within_four_blocks_height)];

pub struct GhastEntity {
    pub mob_entity: MobEntity,
    pub is_charging: AtomicBool,
    pub explosion_power: AtomicU8,
}

impl GhastEntity {
    pub const DEFAULT_EXPLOSION_POWER: u8 = 1;
    pub const XP_REWARD: u32 = 5;
    /// Vanilla `Ghast.travel`: `travelFlying(input, 0.02F)`.
    const TRAVEL_SPEED: f32 = 0.02;

    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Ghast.defineSynchedData`.
        entity
            .synched_data
            .define(tracked_data::ghast::DATA_IS_CHARGING, false);
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(GhastMoveControl::new(false, |_| false));
        let ghast = Self {
            mob_entity,
            is_charging: AtomicBool::new(false),
            explosion_power: AtomicU8::new(Self::DEFAULT_EXPLOSION_POWER),
        };

        let mob_arc = Arc::new(ghast);

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Priority 5: Random floating around
            goal_selector.add_goal(5, Box::new(RandomFloatAroundGoal::new(0)));

            // Priority 7: Face target / movement direction
            goal_selector.add_goal(7, Box::new(GhastLookGoal::new()));

            // Priority 7: Shoot large fireballs at target
            goal_selector.add_goal(
                7,
                Box::new(GhastShootFireballGoal::new(Arc::downgrade(&mob_arc))),
            );

            let target_selector = &mob_arc.mob_entity.target_selector;

            // Priority 1: Target nearest player within vertical proximity
            hunting::apply(target_selector, &mob_arc.mob_entity, GHAST_HUNTS);
        };

        mob_arc
    }

    pub fn set_charging(&self, charging: bool) {
        self.is_charging.store(charging, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_synced_data(tracked_data::ghast::DATA_IS_CHARGING, charging);
    }

    #[must_use]
    pub fn is_charging(&self) -> bool {
        self.is_charging.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn get_explosion_power(&self) -> u8 {
        self.explosion_power.load(Ordering::Relaxed)
    }

    pub fn set_explosion_power(&self, power: u8) {
        self.explosion_power.store(power, Ordering::Relaxed);
    }

    #[must_use]
    pub fn check_ghast_spawn_rules(world: &World, pos: &BlockPos) -> bool {
        if world.level_info.load().difficulty == pumpkin_util::Difficulty::Peaceful {
            return false;
        }
        if game_random().random_range(0..20) != 0 {
            return false;
        }
        let state = world.get_block_state(pos);
        state.is_air()
    }
}

impl Mob for GhastEntity {
    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    fn mob_support_quad_leash_as_holder(&self) -> bool {
        true
    }

    fn mob_leash_elastic_distance(&self) -> f64 {
        LEASH_ELASTIC_DISTANCE
    }

    fn mob_leash_snap_distance(&self) -> f64 {
        LEASH_SNAP_DISTANCE
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn on_climbable(&self) -> bool {
        false
    }

    fn should_check_fall_damage(&self) -> bool {
        false
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0 // Ghasts fly, no gravity applied in standard travel
    }

    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.95)
    }

    fn travel(&self, caller: &dyn EntityBase) -> bool {
        self.mob_entity
            .living_entity
            .travel_flying_at(caller, f64::from(Self::TRAVEL_SPEED));
        true
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if self.is_charging() {
            entity.set_synced_data(tracked_data::ghast::DATA_IS_CHARGING, true);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let power = self.get_explosion_power();
        nbt.put_byte("ExplosionPower", i8::try_from(power).unwrap_or(1));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(power) = nbt.get_byte("ExplosionPower") {
            self.set_explosion_power(u8::try_from(power).unwrap_or(Self::DEFAULT_EXPLOSION_POWER));
        }
    }

    /// Vanilla `Ghast.hurtServer`: a ghast fireball a player sent back kills it, despite its fire
    /// immunity.
    fn mob_hurt_server(
        &self,
        caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        let reflected = source.is_some_and(|source| source.cast_any().is::<FireballEntity>())
            && cause.is_some_and(|cause| cause.get_player().is_some());
        if !reflected {
            return None;
        }
        // Vanilla `Ghast.isInvulnerableTo` lets the reflected fireball past everything except
        // the `Invulnerable` tag.
        let living = &self.mob_entity.living_entity;
        let invulnerable = living.entity.invulnerable.load(Ordering::Relaxed)
            && !damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY);
        if !invulnerable {
            living.damage_ignoring_invulnerability(
                caller,
                REFLECTED_FIREBALL_DAMAGE,
                damage_type,
                position,
                source,
                cause,
            );
        }
        Some(true)
    }

    fn xp_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}

/// Vanilla `Ghast.faceMovementDirection`: faces the target within 64 blocks, otherwise the
/// direction of travel.
pub fn face_movement_direction(mob: &dyn Mob) {
    let mob_entity = mob.get_mob_entity();
    let entity = &mob_entity.living_entity.entity;
    if let Some(target) = mob_entity.get_target() {
        let pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        if target_pos.squared_distance_to_vec(&pos) < 4096.0 {
            let yaw = -(f64::atan2(target_pos.x - pos.x, target_pos.z - pos.z) as f32).to_degrees();
            entity.yaw.store(yaw);
            entity.body_yaw.store(yaw);
        }
    } else {
        let movement = entity.velocity.load();
        let yaw = -(f64::atan2(movement.x, movement.z) as f32).to_degrees();
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
    }
}

pub struct GhastLookGoal {
    goal_control: Controls,
}

impl Default for GhastLookGoal {
    fn default() -> Self {
        Self {
            goal_control: Controls::LOOK,
        }
    }
}

impl GhastLookGoal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Goal for GhastLookGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        true
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        face_movement_direction(mob);
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

pub struct GhastShootFireballGoal {
    ghast: Weak<GhastEntity>,
    charge_time: i32,
}

impl GhastShootFireballGoal {
    #[must_use]
    pub const fn new(ghast: Weak<GhastEntity>) -> Self {
        Self {
            ghast,
            charge_time: 0,
        }
    }
}

impl Goal for GhastShootFireballGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(ghast) = self.ghast.upgrade() else {
            return false;
        };
        let target = ghast.mob_entity.get_target();
        target.is_some_and(|t| t.get_entity().is_alive())
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(ghast) = self.ghast.upgrade() else {
            return false;
        };
        let target = ghast.mob_entity.get_target();
        target.is_some_and(|t| t.get_entity().is_alive())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.charge_time = 0;
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(ghast) = self.ghast.upgrade() {
            ghast.set_charging(false);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(ghast) = self.ghast.upgrade() else {
            return;
        };

        let target_opt = ghast.mob_entity.get_target();
        let Some(target) = target_opt else {
            return;
        };

        let entity = &ghast.mob_entity.living_entity.entity;
        let ghast_pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        let dist_sq = ghast_pos.squared_distance_to_vec(&target_pos);

        if dist_sq < 4096.0 {
            let world = entity.world.load();
            self.charge_time += 1;

            if self.charge_time == 10 {
                world.play_sound_fine(
                    Sound::EntityGhastWarn,
                    SoundCategory::Hostile,
                    &ghast_pos,
                    5.0,
                    1.0,
                );
            }

            if self.charge_time == 20 {
                world.play_sound_fine(
                    Sound::EntityGhastShoot,
                    SoundCategory::Hostile,
                    &ghast_pos,
                    5.0,
                    1.0,
                );

                let yaw_rad = f64::from(entity.yaw.load()).to_radians();
                let pitch_rad = f64::from(entity.pitch.load()).to_radians();
                let view_x = -pitch_rad.cos() * yaw_rad.sin();
                let view_z = pitch_rad.cos() * yaw_rad.cos();

                let spawn_pos = Vector3::new(
                    ghast_pos.x + view_x * 4.0,
                    ghast_pos.y + 2.5,
                    ghast_pos.z + view_z * 4.0,
                );

                let target_y = target_pos.y + target.get_entity().get_eye_height() * 0.5;
                let dir_x = target_pos.x - spawn_pos.x;
                let dir_y = target_y - spawn_pos.y;
                let dir_z = target_pos.z - spawn_pos.z;
                let direction = Vector3::new(dir_x, dir_y, dir_z);

                let fireball_base = Entity::from_uuid(
                    uuid::Uuid::new_v4(),
                    world.clone(),
                    spawn_pos,
                    &EntityType::FIREBALL,
                );

                let fireball = FireballEntity::new_shot(fireball_base, entity, direction);
                fireball
                    .explosion_power
                    .store(f32::from(ghast.get_explosion_power()), Ordering::Relaxed);

                world.spawn_entity_non_save(Arc::new(fireball));
                self.charge_time = -40;
            }
        } else if self.charge_time > 0 {
            self.charge_time -= 1;
        }

        ghast.set_charging(self.charge_time > 10);
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `Ghast.GhastMoveControl`: pushes the mob towards the wanted position every few ticks.
///
/// It gives up when the way there is blocked. A careful one (the happy ghast) also keeps a block
/// of clearance and avoids fluids and `happy_ghast_avoids` blocks.
pub struct GhastMoveControl {
    wanted: Vector3<f64>,
    speed_modifier: f64,
    operation: Operation,
    float_duration: i32,
    careful: bool,
    should_be_stopped: fn(&dyn Mob) -> bool,
}

impl GhastMoveControl {
    #[must_use]
    pub const fn new(careful: bool, should_be_stopped: fn(&dyn Mob) -> bool) -> Self {
        Self {
            wanted: Vector3::new(0.0, 0.0, 0.0),
            speed_modifier: 0.0,
            operation: Operation::Wait,
            float_duration: 0,
            careful,
            should_be_stopped,
        }
    }

    fn can_reach(&self, mob: &dyn Mob, travel: Vector3<f64>) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let aabb = entity.bounding_box.load();
        let aabb_at_destination = aabb.shift(travel);
        if self.careful {
            let inflated = aabb_at_destination.expand_all(1.0);
            for pos in BlockPos::iterate(
                BlockPos::floored_v(inflated.min),
                BlockPos::floored_v(inflated.max),
            ) {
                if !self.block_traversal_possible(entity, &world, None, pos, false, false) {
                    return false;
                }
            }
        }

        let in_water = entity.is_in_water();
        let in_lava = entity.touching_lava.load(Ordering::Relaxed);
        let start = entity.pos.load();
        let end = start + travel;
        for_each_block_intersected_between(start, end, &aabb_at_destination, |pos| {
            aabb.intersects(&BoundingBox::from_block(&pos))
                || self.block_traversal_possible(
                    entity,
                    &world,
                    Some((start, end)),
                    pos,
                    in_water,
                    in_lava,
                )
        })
    }

    fn block_traversal_possible(
        &self,
        entity: &Entity,
        world: &World,
        path: Option<(Vector3<f64>, Vector3<f64>)>,
        pos: BlockPos,
        can_path_through_water: bool,
        can_path_through_lava: bool,
    ) -> bool {
        let (block, state) = world.get_block_and_state(&pos);
        if state.is_air() {
            return true;
        }
        let mut shapes = state
            .get_block_collision_shapes_at(&pos)
            .map(|shape| shape.at_pos(pos));
        let path_no_collisions = match path {
            Some((start, end)) => {
                let shapes: Vec<BoundingBox> = shapes.collect();
                !collided_with_shape_moving_from(entity, start, end, &shapes)
            }
            None => shapes.next().is_none(),
        };
        if !self.careful {
            return path_no_collisions;
        }
        if block.has_tag(&tag::Block::MINECRAFT_HAPPY_GHAST_AVOIDS) {
            return false;
        }
        let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&pos);
        if !fluid_state.is_empty {
            let collided = path.is_none_or(|(start, end)| {
                let height = f64::from(world.get_fluid_height(&pos, fluid, &fluid_state));
                let min = pos.to_f64();
                let fluid_box = BoundingBox::new(min, min + Vector3::new(1.0, height, 1.0));
                collided_with_shape_moving_from(entity, start, end, &[fluid_box])
            });
            if collided {
                if fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
                    return can_path_through_water;
                }
                if fluid.has_tag(&tag::Fluid::MINECRAFT_LAVA) {
                    return can_path_through_lava;
                }
            }
        }
        path_no_collisions
    }
}

impl Control for GhastMoveControl {}

impl MoveControlTrait for GhastMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if (self.should_be_stopped)(mob) {
            self.operation = Operation::Wait;
            mob.get_mob_entity().stop_in_place();
        }

        if self.operation == Operation::MoveTo {
            let float_duration = self.float_duration;
            self.float_duration -= 1;
            if float_duration <= 0 {
                self.float_duration += mob.get_random().random_range(0..5) + 2;
                let entity = mob.get_entity();
                let travel = self.wanted - entity.pos.load();
                if self.can_reach(mob, travel) {
                    let flying_speed = mob
                        .get_mob_entity()
                        .living_entity
                        .get_attribute_value(&Attributes::FLYING_SPEED);
                    entity.velocity.store(
                        entity.velocity.load() + normalize(travel) * (flying_speed * 5.0 / 3.0),
                    );
                } else {
                    self.operation = Operation::Wait;
                }
            }
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted = Vector3::new(x, y, z);
        self.speed_modifier = speed_modifier;
        self.operation = Operation::MoveTo;
    }

    fn set_wait(&mut self) {
        self.operation = Operation::Wait;
    }

    fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }

    fn get_speed_modifier(&self) -> f64 {
        self.speed_modifier
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        self.wanted
    }
}

/// Vanilla `Vec3.normalize`: too short a vector becomes zero.
fn normalize(vector: Vector3<f64>) -> Vector3<f64> {
    let length = vector.length();
    if length < f64::from(1.0E-5f32) {
        Vector3::new(0.0, 0.0, 0.0)
    } else {
        vector * (1.0 / length)
    }
}

/// Vanilla `Entity.collidedWithShapeMovingFrom` with `AABB.collidedAlongVector`.
fn collided_with_shape_moving_from(
    entity: &Entity,
    from: Vector3<f64>,
    to: Vector3<f64>,
    shapes: &[BoundingBox],
) -> bool {
    let width = f64::from(entity.width());
    let height = f64::from(entity.height());
    let center = from + Vector3::new(0.0, height / 2.0, 0.0);
    let travel = to - from;
    let target = center + travel;
    shapes.iter().any(|shape| {
        let inflated = shape.expand(
            width * 0.5 - 1.0E-7,
            height * 0.5 - 1.0E-7,
            width * 0.5 - 1.0E-7,
        );
        contains(&inflated, target)
            || contains(&inflated, center)
            || inflated.ray_intersection(center, travel).is_some()
    })
}

/// Vanilla `AABB.contains`: the min faces count, the max faces don't.
fn contains(aabb: &BoundingBox, point: Vector3<f64>) -> bool {
    point.x >= aabb.min.x
        && point.x < aabb.max.x
        && point.y >= aabb.min.y
        && point.y < aabb.max.y
        && point.z >= aabb.min.z
        && point.z < aabb.max.z
}

/// Vanilla `BlockPos.betweenClosed(AABB)`.
fn between_closed(aabb: &BoundingBox) -> impl Iterator<Item = BlockPos> {
    BlockPos::iterate(BlockPos::floored_v(aabb.min), BlockPos::floored_v(aabb.max))
}

/// Vanilla `BlockGetter.forEachBlockIntersectedBetween`: visits each block the box touches on
/// its way from `from` to `to` (where it is `aabb_at_target`) until `visitor` returns false.
fn for_each_block_intersected_between(
    from: Vector3<f64>,
    to: Vector3<f64>,
    aabb_at_target: &BoundingBox,
    mut visitor: impl FnMut(BlockPos) -> bool,
) -> bool {
    let travel = to - from;
    if travel.length_squared() < f64::from(1.0E-5f32 * 1.0E-5f32) {
        return between_closed(aabb_at_target).all(&mut visitor);
    }

    let mut visited = FxHashSet::default();
    for pos in between_closed(&aabb_at_target.shift(travel * -1.0)) {
        if !visitor(pos) {
            return false;
        }
        visited.insert(pos);
    }

    if !add_collisions_along_travel(&mut visited, travel, aabb_at_target, &mut visitor) {
        return false;
    }

    between_closed(aabb_at_target).all(|pos| !visited.insert(pos) || visitor(pos))
}

const fn sign(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// Vanilla `BlockGetter.addCollisionsAlongTravel`: steps the box's leading corner through the
/// blocks between the start and the target box.
fn add_collisions_along_travel(
    visited: &mut FxHashSet<BlockPos>,
    delta: Vector3<f64>,
    aabb_at_target: &BoundingBox,
    visitor: &mut impl FnMut(BlockPos) -> bool,
) -> bool {
    let size = aabb_at_target.max - aabb_at_target.min;
    let corner = furthest_corner(delta);
    let center = aabb_at_target.get_center();
    let to_corner = Vector3::new(
        center.x + size.x * 0.5 * f64::from(corner.x),
        center.y + size.y * 0.5 * f64::from(corner.y),
        center.z + size.z * 0.5 * f64::from(corner.z),
    );
    let from_corner = to_corner - delta;
    let mut block = BlockPos::floored_v(from_corner).0;
    let (sign_x, sign_y, sign_z) = (sign(delta.x), sign(delta.y), sign(delta.z));
    let t_delta = |sign: i32, delta: f64| {
        if sign == 0 {
            f64::MAX
        } else {
            f64::from(sign) / delta
        }
    };
    let (t_delta_x, t_delta_y, t_delta_z) = (
        t_delta(sign_x, delta.x),
        t_delta(sign_y, delta.y),
        t_delta(sign_z, delta.z),
    );
    let t_start = |sign: i32, t_delta: f64, coord: f64| {
        let frac = coord - coord.floor();
        t_delta * if sign > 0 { 1.0 - frac } else { frac }
    };
    let mut t_x = t_start(sign_x, t_delta_x, from_corner.x);
    let mut t_y = t_start(sign_y, t_delta_y, from_corner.y);
    let mut t_z = t_start(sign_z, t_delta_z, from_corner.z);
    let corner_travel = to_corner - from_corner;
    let epsilon = f64::from(1.0E-5f32);

    while t_x <= 1.0 || t_y <= 1.0 || t_z <= 1.0 {
        if t_x < t_y {
            if t_x < t_z {
                block.x += sign_x;
                t_x += t_delta_x;
            } else {
                block.z += sign_z;
                t_z += t_delta_z;
            }
        } else if t_y < t_z {
            block.y += sign_y;
            t_y += t_delta_y;
        } else {
            block.z += sign_z;
            t_z += t_delta_z;
        }

        let block_pos = BlockPos(block);
        let Some(t) =
            BoundingBox::from_block(&block_pos).ray_intersection(from_corner, corner_travel)
        else {
            continue;
        };
        let hit = from_corner + corner_travel * t;
        let (bx, by, bz) = (f64::from(block.x), f64::from(block.y), f64::from(block.z));
        let opposite = BlockPos::floored_v(Vector3::new(
            hit.x.clamp(bx + epsilon, bx + 1.0 - epsilon) - size.x * f64::from(corner.x),
            hit.y.clamp(by + epsilon, by + 1.0 - epsilon) - size.y * f64::from(corner.y),
            hit.z.clamp(bz + epsilon, bz + 1.0 - epsilon) - size.z * f64::from(corner.z),
        ));
        let min = BlockPos::new(
            block.x.min(opposite.0.x),
            block.y.min(opposite.0.y),
            block.z.min(opposite.0.z),
        );
        let max = BlockPos::new(
            block.x.max(opposite.0.x),
            block.y.max(opposite.0.y),
            block.z.max(opposite.0.z),
        );
        for pos in BlockPos::iterate(min, max) {
            if visited.insert(pos) && !visitor(pos) {
                return false;
            }
        }
    }
    true
}

/// Vanilla `BlockGetter.getFurthestCorner`.
fn furthest_corner(direction: Vector3<f64>) -> Vector3<i32> {
    let x_dot = direction.x.abs();
    let y_dot = direction.y.abs();
    let z_dot = direction.z.abs();
    let x_sign = if direction.x >= 0.0 { 1 } else { -1 };
    let y_sign = if direction.y >= 0.0 { 1 } else { -1 };
    let z_sign = if direction.z >= 0.0 { 1 } else { -1 };
    if x_dot <= y_dot && x_dot <= z_dot {
        Vector3::new(-x_sign, -z_sign, y_sign)
    } else if y_dot <= z_dot {
        Vector3::new(z_sign, -y_sign, -x_sign)
    } else {
        Vector3::new(-y_sign, x_sign, -z_sign)
    }
}

/// Vanilla `Ghast.RandomFloatAroundGoal`: picks a new spot once the old one is reached or far.
///
/// With `distance_to_blocks`, the spot must have a block within that distance (the happy ghast
/// stays near the ground).
pub struct RandomFloatAroundGoal {
    distance_to_blocks: i32,
}

impl RandomFloatAroundGoal {
    const MAX_ATTEMPTS: usize = 64;

    #[must_use]
    pub const fn new(distance_to_blocks: i32) -> Self {
        Self { distance_to_blocks }
    }

    /// Vanilla `getSuitableFlyToPosition`.
    pub fn get_suitable_fly_to_position(mob: &dyn Mob, distance_to_blocks: i32) -> Vector3<f64> {
        let mob_entity = mob.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        let center = entity.pos.load();
        let mut result = None;
        for _ in 0..Self::MAX_ATTEMPTS {
            let target = Self::choose_random_position(mob, center);
            result = (!mob_entity.has_position_target()
                || mob_entity.is_in_position_target_range_pos(&BlockPos::floored_v(target)))
            .then_some(target);
            if let Some(target) = result
                && Self::is_good_target(&world, target, distance_to_blocks)
            {
                return target;
            }
        }

        let mut result = result.unwrap_or_else(|| Self::choose_random_position(mob, center));
        let pos = BlockPos::floored_v(result);
        let height_y = world.get_heightmap_height(MotionBlocking, pos.0.x, pos.0.z) + 1;
        if height_y < pos.0.y && height_y > world.min_y {
            result = Vector3::new(result.x, center.y - (center.y - result.y).abs(), result.z);
        }
        result
    }

    fn is_good_target(world: &World, target: Vector3<f64>, distance_to_blocks: i32) -> bool {
        if distance_to_blocks <= 0 {
            return true;
        }
        let pos = BlockPos::floored_v(target);
        if !world.get_block_state(&pos).is_air() {
            return false;
        }
        BlockDirection::all().into_iter().any(|direction| {
            let offset = direction.to_offset();
            (1..distance_to_blocks).any(|i| {
                let offset = Vector3::new(offset.x * i, offset.y * i, offset.z * i);
                !world.get_block_state(&pos.offset(offset)).is_air()
            })
        })
    }

    fn choose_random_position(mob: &dyn Mob, center: Vector3<f64>) -> Vector3<f64> {
        let rng = mob.get_random();
        let offset = || f64::from((rng.next_f32() * 2.0 - 1.0) * 16.0);
        let x = center.x + offset();
        let y = center.y + offset();
        let z = center.z + offset();
        Vector3::new(x, y, z)
    }
}

impl Goal for RandomFloatAroundGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let mob_entity = mob.get_mob_entity();
        let steered = mob_entity.steered();
        let move_control = steered.move_control();
        if !move_control.has_wanted() {
            return true;
        }
        let dd = move_control
            .get_wanted_position()
            .squared_distance_to_vec(&mob_entity.living_entity.entity.pos.load());
        !(1.0..=3600.0).contains(&dd)
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let result = Self::get_suitable_fly_to_position(mob, self.distance_to_blocks);
        mob.get_mob_entity()
            .steered()
            .move_control()
            .set_wanted_position(result.x, result.y, result.z, 1.0);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

impl crate::entity::custom_sound::CustomSound for GhastEntity {
    fn death_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        None
    }

    fn hurt_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        None
    }

    fn sound_volume(&self) -> f32 {
        5.0
    }
}
