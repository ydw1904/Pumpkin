use core::f32;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::{damage::DamageType, entity::EntityType};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{
    boundingbox::BoundingBox,
    vector3::{Axis, Vector3},
};

use crate::{
    block::blocks::bubble_column::SURFACE_UPWARD_ACCELERATION,
    server::Server,
    world::{
        World,
        entity_islands::{self, AuditedReach},
    },
};

use super::{Entity, EntityBase, living::LivingEntity, player::Player};

pub struct ExperienceOrbEntity {
    entity: Entity,
    value: AtomicI32,
    health: AtomicI32,
    age: AtomicI32,
    /// How many orbs of this value merged into this one; each touch takes one.
    count: AtomicI32,
    following_player: Mutex<Weak<Player>>,
}

impl ExperienceOrbEntity {
    const AIR_DRAG: f32 = 0.98;
    const DEFAULT_HEALTH: i32 = 5;
    const LIFETIME: i32 = 6000;
    const ENTITY_SCAN_PERIOD: i32 = 20;
    const MAX_FOLLOW_DIST: f64 = 8.0;
    const ORB_GROUPS_PER_AREA: i32 = 40;
    const ORB_MERGE_DISTANCE: f64 = 0.5;
    /// Largest speed `followNearbyPlayer` adds in a tick.
    const MAX_FOLLOW_ACCELERATION: f64 = 0.1;
    /// Largest speed per axis of the bounce out of lava.
    const LAVA_BOUNCE_SPEED: f64 = 0.2;

    pub fn new(entity: Entity, value: u32) -> Self {
        entity.yaw.store(game_random().random::<f32>() * 360.0);
        Self {
            entity,
            value: AtomicI32::new(value as i32),
            health: AtomicI32::new(Self::DEFAULT_HEALTH),
            age: AtomicI32::new(0),
            count: AtomicI32::new(1),
            following_player: Mutex::new(Weak::new()),
        }
    }

    fn value(&self) -> i32 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn spawn(world: &Arc<World>, position: Vector3<f64>, amount: u32) {
        Self::spawn_with_direction(world, position, Vector3::new(0.0, 0.0, 0.0), amount);
    }

    fn new_with_direction(
        world: &Arc<World>,
        position: Vector3<f64>,
        direction: Vector3<f64>,
        amount: u32,
    ) -> Self {
        let entity = Entity::new(world.clone(), position, &EntityType::EXPERIENCE_ORB);
        let orb = Self::new(entity, amount);
        let mut velocity = Vector3::new(
            (game_random().random::<f64>() * 0.2 - 0.1) * 2.0,
            game_random().random::<f64>() * 0.2 * 2.0,
            (game_random().random::<f64>() * 0.2 - 0.1) * 2.0,
        );
        if direction.length_squared() > 0.0 && direction.dot(&velocity) < 0.0 {
            velocity = velocity.multiply(-1.0, -1.0, -1.0);
        }
        let size = orb.entity.bounding_box.load().get_average_side_length();
        let offset = direction.normalize() * (size * 0.5);
        orb.entity.set_pos(position.add(&offset));
        orb.entity.set_velocity(velocity);
        if !world.is_space_empty(orb.entity.bounding_box.load()) {
            orb.unstuck_if_possible(size);
        }
        orb
    }

    fn unstuck_if_possible(&self, max_distance: f64) {
        let bounding_box = self.entity.bounding_box.load();
        let size = bounding_box.max - bounding_box.min;
        let center = (bounding_box.min + bounding_box.max) * 0.5;
        let half_distance = Vector3::new(max_distance, max_distance, max_distance) * 0.5;
        let allowed_centers = BoundingBox {
            min: center - half_distance,
            max: center + half_distance,
        };
        let world = self.entity.world.load();
        let (mut collisions, _) =
            world.get_block_collisions(allowed_centers.expand(size.x, size.y, size.z), self);
        {
            let border = world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let half = border.new_diameter * 0.5;
            let epsilon = f64::from(1.0e-5f32);
            collisions.retain(|shape| {
                shape.min.x >= border.center_x - half
                    && shape.max.x - epsilon < border.center_x + half
                    && shape.min.z >= border.center_z - half
                    && shape.max.z - epsilon < border.center_z + half
            });
        };
        let expanded_collisions = collisions
            .into_iter()
            .map(|shape| shape.expand(size.x * 0.5, size.y * 0.5, size.z * 0.5));
        if let Some(position) = closest_free_center(allowed_centers, center, expanded_collisions) {
            self.entity
                .set_pos(position.add_raw(0.0, -size.y * 0.5, 0.0));
        }
    }

    pub fn spawn_with_direction(
        world: &Arc<World>,
        position: Vector3<f64>,
        direction: Vector3<f64>,
        amount: u32,
    ) {
        let mut amount = amount;
        while amount > 0 {
            let i = Self::round_to_orb_size(amount);
            amount -= i;
            if !Self::try_merge_to_existing(world, position, i as i32) {
                let orb = Arc::new(Self::new_with_direction(world, position, direction, i));
                world.spawn_entity(orb);
            }
        }
    }

    /// Vanilla `ExperienceOrb.tryMergeToExisting`: adds to an orb of the same value and group
    /// within half a block instead of spawning another one.
    fn try_merge_to_existing(world: &Arc<World>, position: Vector3<f64>, value: i32) -> bool {
        let half = Vector3::new(0.5, 0.5, 0.5);
        let area = BoundingBox::new(position - half, position + half);
        let group = game_random().next_bounded_i32(Self::ORB_GROUPS_PER_AREA);
        let found = world.get_entities_at_box(&area).into_iter().find(|other| {
            other
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(|orb| orb.can_merge(group, value))
        });
        let Some(found) = found else {
            return false;
        };
        if let Some(orb) = found.cast_any().downcast_ref::<Self>() {
            orb.count.fetch_add(1, Ordering::Relaxed);
            orb.age.store(0, Ordering::Relaxed);
        }
        true
    }

    fn can_merge(&self, group: i32, value: i32) -> bool {
        !self.entity.removed.load(Ordering::Relaxed)
            && (self.entity.entity_id - group) % Self::ORB_GROUPS_PER_AREA == 0
            && self.value() == value
    }

    /// Vanilla `ExperienceOrb.scanForMerges`.
    fn scan_for_merges(&self) {
        let area = self.entity.bounding_box.load().expand(
            Self::ORB_MERGE_DISTANCE,
            Self::ORB_MERGE_DISTANCE,
            Self::ORB_MERGE_DISTANCE,
        );
        let world = self.entity.world.load();
        for other in world.get_entities_at_box(&area) {
            let Some(orb) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            if orb.entity.entity_id != self.entity.entity_id
                && orb.can_merge(self.entity.entity_id, self.value())
            {
                self.count
                    .fetch_add(orb.count.load(Ordering::Relaxed), Ordering::Relaxed);
                self.age
                    .fetch_min(orb.age.load(Ordering::Relaxed), Ordering::Relaxed);
                orb.entity.remove();
            }
        }
    }

    /// Vanilla `ExperienceOrb.followNearbyPlayer`.
    fn follow_nearby_player(&self) {
        let entity = &self.entity;
        let pos = entity.pos.load();
        let world = entity.world.load_full();
        let mut following = self
            .following_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keep = following.upgrade().filter(|player| {
            !player.is_spectator()
                && Arc::ptr_eq(&player.world(), &world)
                && player.get_entity().pos.load().squared_distance_to_vec(&pos)
                    <= Self::MAX_FOLLOW_DIST * Self::MAX_FOLLOW_DIST
        });
        let player = keep.or_else(|| {
            world
                .get_nearest_player(pos, Self::MAX_FOLLOW_DIST, |player| !player.is_spectator())
                .filter(|player| player.living_entity.health.load() > 0.0)
        });
        *following = player.as_ref().map_or_else(Weak::new, Arc::downgrade);
        drop(following);

        if let Some(player) = player {
            let player_entity = player.get_entity();
            let player_pos = player_entity.pos.load();
            let delta = Vector3::new(
                player_pos.x - pos.x,
                player_pos.y + player_entity.get_eye_height() / 2.0 - pos.y,
                player_pos.z - pos.z,
            );
            let power = 1.0 - delta.length_squared().sqrt() / Self::MAX_FOLLOW_DIST;
            entity.velocity.store(
                entity.velocity.load()
                    + delta.normalize() * (power * power * Self::MAX_FOLLOW_ACCELERATION),
            );
        }
    }

    const fn round_to_orb_size(value: u32) -> u32 {
        if value >= 2477 {
            2477
        } else if value >= 1237 {
            1237
        } else if value >= 617 {
            617
        } else if value >= 307 {
            307
        } else if value >= 149 {
            149
        } else if value >= 73 {
            73
        } else if value >= 37 {
            37
        } else if value >= 17 {
            17
        } else if value >= 7 {
            7
        } else if value >= 3 {
            3
        } else {
            1
        }
    }
}

fn closest_free_center(
    allowed_centers: BoundingBox,
    center: Vector3<f64>,
    collisions: impl Iterator<Item = BoundingBox>,
) -> Option<Vector3<f64>> {
    // Subtract expanded collision boxes, matching vanilla's ONLY_FIRST shape operation.
    let mut free_boxes = vec![allowed_centers];
    for collision in collisions {
        let mut remaining = Vec::new();
        for mut free in free_boxes {
            if !free.intersects(&collision) {
                remaining.push(free);
                continue;
            }
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                if free.min.get_axis(axis) < collision.min.get_axis(axis) {
                    let mut piece = free;
                    piece.max.set_axis(axis, collision.min.get_axis(axis));
                    remaining.push(piece);
                    free.min.set_axis(axis, collision.min.get_axis(axis));
                }
                if free.max.get_axis(axis) > collision.max.get_axis(axis) {
                    let mut piece = free;
                    piece.min.set_axis(axis, collision.max.get_axis(axis));
                    remaining.push(piece);
                    free.max.set_axis(axis, collision.max.get_axis(axis));
                }
            }
        }
        free_boxes = remaining;
    }
    free_boxes
        .into_iter()
        .map(|free| {
            Vector3::new(
                center.x.clamp(free.min.x, free.max.x),
                center.y.clamp(free.min.y, free.max.y),
                center.z.clamp(free.min.z, free.max.z),
            )
        })
        .min_by(|a, b| {
            a.squared_distance_to_vec(&center)
                .total_cmp(&b.squared_distance_to_vec(&center))
        })
}

impl EntityBase for ExperienceOrbEntity {
    /// Vanilla `ExperienceOrb.doWaterSplashEffect` does nothing.
    fn do_water_splash_effect(&self, _caller: &dyn EntityBase) {}

    fn audited_reach(&self) -> Option<AuditedReach> {
        let entity = &self.entity;
        let world = entity.world.load();
        let fluid_push = super::max_fluid_push_speed(world.dimension.fast_lava);
        let velocity = entity.velocity.load();
        // Following a player, the lava bounce or a push out of blocks can each change the motion.
        let steer = Self::MAX_FOLLOW_ACCELERATION
            .max(Self::LAVA_BOUNCE_SPEED * std::f64::consts::SQRT_2)
            .max(super::MAX_PUSH_OUT_SPEED);
        let mut links = entity_islands::links_of(self);
        if let Some(player) = self
            .following_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
        {
            links.push(player.entity_id());
        }
        let mut reach = AuditedReach {
            bounding_box: entity.bounding_box.load(),
            motion: (
                velocity.x.hypot(velocity.z) + fluid_push + steer,
                velocity.y.abs()
                    + self.get_gravity()
                    + SURFACE_UPWARD_ACCELERATION
                    + fluid_push
                    + steer,
            ),
            entities: Self::ORB_MERGE_DISTANCE,
            players: Self::MAX_FOLLOW_DIST,
            block_reads: 1.0,
            route: None,
            changes_blocks: false,
            global: false,
            links,
        };
        reach.scan_contact_blocks(&world);
        Some(reach)
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_short("Health", self.health.load(Ordering::Relaxed) as i16);
        nbt.put_short("Age", self.age.load(Ordering::Relaxed) as i16);
        nbt.put_short("Value", self.value() as i16);
        nbt.put_int("Count", self.count.load(Ordering::Relaxed));
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.health.store(
            nbt.get_numeric_short("Health")
                .map_or(Self::DEFAULT_HEALTH, i32::from),
            Ordering::Relaxed,
        );
        self.age.store(
            nbt.get_numeric_short("Age").map_or(0, i32::from),
            Ordering::Relaxed,
        );
        self.value.store(
            nbt.get_numeric_short("Value").map_or(0, i32::from),
            Ordering::Relaxed,
        );
        self.count.store(
            nbt.get_int("Count").filter(|count| *count > 0).unwrap_or(1),
            Ordering::Relaxed,
        );
    }

    fn init_data_tracker(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::experience_orb::DATA_VALUE,
            VarInt(self.value()),
        );
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        entity.tick(caller, server);
        let bounding_box = entity.bounding_box.load();

        let world = entity.world.load_full();
        let colliding = !world.is_space_empty(bounding_box.expand(-1.0e-7, -1.0e-7, -1.0e-7));
        let mut velo = entity.velocity.load();
        if entity.is_submerged_in_water() {
            velo = Vector3::new(
                velo.x * f64::from(0.99f32),
                (velo.y + f64::from(5.0e-4f32)).min(f64::from(0.06f32)),
                velo.z * f64::from(0.99f32),
            );
        } else if !colliding {
            velo.y -= self.get_gravity();
        }
        if world
            .get_fluid(&entity.block_pos.load())
            .matches_type(&pumpkin_data::fluid::Fluid::LAVA)
        {
            let rng = game_random();
            let x = (rng.next_f32() - rng.next_f32()) * 0.2;
            let z = (rng.next_f32() - rng.next_f32()) * 0.2;
            velo = Vector3::new(f64::from(x), f64::from(0.2f32), f64::from(z));
        }
        entity.velocity.store(velo);

        if entity.tick_count.load(Ordering::Relaxed) % Self::ENTITY_SCAN_PERIOD == 1 {
            self.scan_for_merges();
        }

        self.follow_nearby_player();
        let following = self
            .following_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
            .is_some();
        if !following && colliding {
            let next = bounding_box.shift(entity.velocity.load());
            if !world.is_space_empty(next.expand(-1.0e-7, -1.0e-7, -1.0e-7)) {
                let pos = entity.pos.load();
                entity.push_out_of_blocks(Vector3::new(
                    pos.x,
                    f64::midpoint(bounding_box.min.y, bounding_box.max.y),
                    pos.z,
                ));
                entity.velocity_dirty.store(true, Ordering::Relaxed);
            }
        }

        let velo = entity.velocity.load();
        entity.move_entity(caller, velo);

        entity.tick_block_collisions(caller);

        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        let mut friction = Self::AIR_DRAG;
        if on_ground {
            let pos = entity.get_pos_with_y_offset(f64::from(0.999_999f32)).0;
            let block = entity.world.load().get_block(&pos);
            friction *= block.slipperiness;
        }
        let mut velocity = entity.velocity.load() * f64::from(friction);
        if on_ground && velo.y < -self.get_gravity() {
            velocity.y = -velo.y * 0.4;
        }
        entity.velocity.store(velocity);

        if self.age.fetch_add(1, Ordering::Relaxed) + 1 >= Self::LIFETIME {
            entity.remove();
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }

        let mut event = crate::plugin::api::events::entity::entity_damage::EntityDamageEvent::new(
            self.entity.entity_id,
            damage_type,
            amount,
        );
        if let Some(server) = self.entity.world.load().server.upgrade() {
            server.plugin_manager.fire_blocking(&server, &mut event);
        }
        if event.cancelled {
            return false;
        }
        if !event.damage.is_finite() || event.damage <= 0.0 {
            return false;
        }

        self.entity.velocity_dirty.store(true, Ordering::Relaxed);
        let mut health = self.health.load(Ordering::Relaxed);
        loop {
            let remaining = (health as f32 - event.damage) as i32;
            match self.health.compare_exchange_weak(
                health,
                remaining,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if remaining <= 0 {
                        self.entity.remove();
                    }
                    return true;
                }
                Err(current) => health = current,
            }
        }
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        if player.living_entity.health.load() > 0.0 {
            let can_pickup = if let Ok(mut delay) = player.experience_pick_up_delay.try_lock()
                && *delay == 0
            {
                *delay = 2;
                true
            } else {
                false
            };
            if can_pickup {
                player.living_entity.pickup(&self.entity, 1);
                let remaining = player.apply_mending_from_xp(self.value());
                if remaining > 0 {
                    player.add_experience_points(remaining);
                }
                if self.count.fetch_sub(1, Ordering::Relaxed) <= 1 {
                    self.entity.remove();
                }
            }
        }
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn get_gravity(&self) -> f64 {
        0.03
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstuck_selects_nearest_free_center_and_leaves_enclosed_orbs_in_place() {
        let allowed = BoundingBox {
            min: Vector3::new(0.0, 0.0, 0.0),
            max: Vector3::new(1.0, 1.0, 1.0),
        };
        let center = Vector3::new(0.5, 0.5, 0.5);
        let wall = BoundingBox {
            min: Vector3::new(-1.0, -1.0, -1.0),
            max: Vector3::new(0.75, 2.0, 2.0),
        };
        let floor = BoundingBox {
            min: Vector3::new(-1.0, -1.0, -1.0),
            max: Vector3::new(2.0, 0.625, 2.0),
        };
        assert_eq!(
            closest_free_center(allowed, center, [wall, floor].into_iter()),
            Some(Vector3::new(0.75, 0.625, 0.5))
        );
        assert_eq!(
            closest_free_center(allowed, center, [allowed].into_iter()),
            None
        );
    }
}
