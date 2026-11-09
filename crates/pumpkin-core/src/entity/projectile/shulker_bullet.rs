use crate::entity::projectile::owner::{EntityReference, ProjectileOwner};
use crate::entity::projectile::{
    ProjectileHit, compute_margin, entity_hit_result, hit_candidates, is_pickable,
    rotate_towards_movement,
};
use crate::entity::projectile_deflection::{
    LastDeflectedBy, ProjectileDeflectionType, redirect_hit_projectile,
};
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::clip::{ClipBlock, ClipContext, ClipFluid};
use pumpkin_world::generation::update_shape::is_face_full;
use uuid::Uuid;

use crate::entity::mob::shulker::Axis;
use crate::entity::{Entity, EntityBase};
use crate::server::Server;
use crate::world::World;
use crate::world::game_event::GameEventContext;

// Direction ordinal constants
const DIR_DOWN: u8 = 0;
const DIR_UP: u8 = 1;
const DIR_NORTH: u8 = 2;
const DIR_SOUTH: u8 = 3;
const DIR_WEST: u8 = 4;
const DIR_EAST: u8 = 5;
const DIR_NONE: u8 = 255;

const SPEED: f64 = 0.15;
/// Vanilla `ShulkerBullet.getDefaultGravity`.
const GRAVITY: f64 = 0.04;
/// Vanilla `ProjectileUtil.rotateTowardsMovement(this, 0.5F)` in `ShulkerBullet.tick`.
const ROTATION_SPEED: f32 = 0.5;
/// Vanilla `ShulkerBullet.onHitEntity` damage.
const HIT_DAMAGE: f32 = 4.0;
/// Vanilla `MobEffectInstance(MobEffects.LEVITATION, 200)` from a hit.
const LEVITATION_TICKS: i32 = 200;

pub struct ShulkerBulletEntity {
    pub entity: Entity,
    pub owner: ProjectileOwner,
    /// Vanilla `finalTarget`.
    target: EntityReference,
    /// Current movement direction (direction ordinal, or `DIR_NONE`)
    current_dir: AtomicU8,
    /// How many ticks remain on the current axis segment
    flight_steps: AtomicI32,
    /// Steering velocity components (updated each tick)
    target_delta_x: AtomicCell<f64>,
    target_delta_y: AtomicCell<f64>,
    target_delta_z: AtomicCell<f64>,
    last_deflected_by: LastDeflectedBy,
}

impl ShulkerBulletEntity {
    pub fn new(owner: &Entity, target: &Entity, invalid_axis: Axis) -> Self {
        let world = owner.world.load();

        // Position at the center of the shulker's bounding box
        let bb = owner.bounding_box.load();
        let origin = Vector3::new(
            f64::midpoint(bb.min.x, bb.max.x),
            f64::midpoint(bb.min.y, bb.max.y),
            f64::midpoint(bb.min.z, bb.max.z),
        );

        let entity = Entity::from_uuid(
            Uuid::new_v4(),
            (*world).clone(),
            origin,
            &EntityType::SHULKER_BULLET,
        );
        entity.no_physics.store(true, Ordering::Relaxed);

        let bullet = Self {
            entity,
            owner: ProjectileOwner::new(Some(owner)),
            target: EntityReference::new(Some(target)),
            current_dir: AtomicU8::new(DIR_UP),
            flight_steps: AtomicI32::new(0),
            target_delta_x: AtomicCell::new(0.0),
            target_delta_y: AtomicCell::new(0.0),
            target_delta_z: AtomicCell::new(0.0),
            last_deflected_by: LastDeflectedBy::new(),
        };

        // Prime target_delta and flight_steps so movement begins on tick 1.
        let avoid_axis = invalid_axis as u8;
        bullet.select_next_dir(avoid_axis, Some(target));

        bullet
    }

    #[allow(clippy::similar_names)]
    fn select_next_dir(&self, avoid_axis: u8, target: Option<&Entity>) {
        let pos = self.entity.pos.load();
        let cur_bp = self.entity.block_pos.load();
        let world = self.entity.world.load();

        // Vanilla aims at the middle of the target's height, or the block below without one.
        let (tbp, y_offset) = target.map_or_else(
            || (BlockPos::new(cur_bp.0.x, cur_bp.0.y - 1, cur_bp.0.z), 0.5),
            |target| {
                let y_offset = f64::from(target.height()) * 0.5;
                let tp = target.pos.load();
                (BlockPos::floored(tp.x, tp.y + y_offset, tp.z), y_offset)
            },
        );

        // ... and the "steering" target point.
        let target_x = f64::from(tbp.0.x) + 0.5;
        let target_y = f64::from(tbp.0.y) + y_offset;
        let target_z = f64::from(tbp.0.z) + 0.5;

        // Vanilla `closerToCenterThan(position, 2.0)`.
        let tbp_cx = f64::from(tbp.0.x) + 0.5;
        let tbp_cy = f64::from(tbp.0.y) + 0.5;
        let tbp_cz = f64::from(tbp.0.z) + 0.5;
        let center_dist_sq =
            (tbp_cx - pos.x).powi(2) + (tbp_cy - pos.y).powi(2) + (tbp_cz - pos.z).powi(2);
        let target_is_far = center_dist_sq >= 4.0;

        let (chosen_tx, chosen_ty, chosen_tz, selected_dir) = if target_is_far {
            // Pick an axis-aligned direction toward the target
            let mut options: Vec<u8> = Vec::new();

            if avoid_axis != 0 {
                if cur_bp.0.x < tbp.0.x && is_empty_block(&world, &cur_bp, DIR_EAST) {
                    options.push(DIR_EAST);
                } else if cur_bp.0.x > tbp.0.x && is_empty_block(&world, &cur_bp, DIR_WEST) {
                    options.push(DIR_WEST);
                }
            }
            if avoid_axis != 1 {
                if cur_bp.0.y < tbp.0.y && is_empty_block(&world, &cur_bp, DIR_UP) {
                    options.push(DIR_UP);
                } else if cur_bp.0.y > tbp.0.y && is_empty_block(&world, &cur_bp, DIR_DOWN) {
                    options.push(DIR_DOWN);
                }
            }
            if avoid_axis != 2 {
                if cur_bp.0.z < tbp.0.z && is_empty_block(&world, &cur_bp, DIR_SOUTH) {
                    options.push(DIR_SOUTH);
                } else if cur_bp.0.z > tbp.0.z && is_empty_block(&world, &cur_bp, DIR_NORTH) {
                    options.push(DIR_NORTH);
                }
            }

            // Without options, a random direction rerolled up to 5 times while it is blocked,
            // ignoring `avoid_axis`.
            let sel = if options.is_empty() {
                let mut r = Self::random_dir();
                for _ in 0..5 {
                    if is_empty_block(&world, &cur_bp, r) {
                        break;
                    }
                    r = Self::random_dir();
                }
                r
            } else {
                options[game_random().random_range(0..options.len())]
            };

            (
                pos.x + f64::from(dir_step_x_s(sel)),
                pos.y + f64::from(dir_step_y_s(sel)),
                pos.z + f64::from(dir_step_z_s(sel)),
                sel,
            )
        } else {
            // If close, set selection=null and aim at the actual target position directly.
            (target_x, target_y, target_z, DIR_NONE)
        };

        // Compute normalized steering delta scaled by SPEED.
        let xa = chosen_tx - pos.x;
        let ya = chosen_ty - pos.y;
        let za = chosen_tz - pos.z;
        let dist = (xa * xa + ya * ya + za * za).sqrt();

        if dist == 0.0 {
            self.target_delta_x.store(0.0);
            self.target_delta_y.store(0.0);
            self.target_delta_z.store(0.0);
        } else {
            self.target_delta_x.store(xa / dist * SPEED);
            self.target_delta_y.store(ya / dist * SPEED);
            self.target_delta_z.store(za / dist * SPEED);
        }

        self.current_dir.store(selected_dir, Ordering::Relaxed);
        let steps = 10 + game_random().random_range(0..5) * 10;
        self.flight_steps.store(steps, Ordering::Relaxed);
    }

    fn random_dir() -> u8 {
        game_random().random_range(0u8..6)
    }
}

// Free-standing step helpers (used inside async closures that can't capture self.*)
const fn dir_step_x_s(dir: u8) -> i32 {
    match dir {
        DIR_EAST => 1,
        DIR_WEST => -1,
        _ => 0,
    }
}
const fn dir_step_y_s(dir: u8) -> i32 {
    match dir {
        DIR_UP => 1,
        DIR_DOWN => -1,
        _ => 0,
    }
}
const fn dir_step_z_s(dir: u8) -> i32 {
    match dir {
        DIR_SOUTH => 1,
        DIR_NORTH => -1,
        _ => 0,
    }
}

/// Returns the axis ordinal (0=X, 1=Y, 2=Z) of a direction.
/// Returns 255 for `DIR_NONE` or any unknown value.
const fn dir_axis(dir: u8) -> u8 {
    match dir {
        DIR_EAST | DIR_WEST => 0,
        DIR_UP | DIR_DOWN => 1,
        DIR_NORTH | DIR_SOUTH => 2,
        _ => 255,
    }
}

/// `true` if the block one step in `dir` from `from` is air (the bullet
/// can pass through it). Unloaded chunks are treated as empty.
fn is_empty_block(world: &crate::world::World, from: &BlockPos, dir: u8) -> bool {
    let nb = BlockPos::new(
        from.0.x + dir_step_x_s(dir),
        from.0.y + dir_step_y_s(dir),
        from.0.z + dir_step_z_s(dir),
    );
    world
        .get_block_state_if_loaded(&nb)
        .is_none_or(pumpkin_data::BlockState::is_air)
}

impl ShulkerBulletEntity {
    /// Create a `ShulkerBulletEntity` from a pre-existing Entity (e.g. loaded from
    /// disk or spawned via the /summon command with no context).
    pub fn orphan(entity: Entity) -> Self {
        entity.no_physics.store(true, Ordering::Relaxed);
        Self {
            entity,
            owner: ProjectileOwner::none(),
            target: EntityReference::none(),
            current_dir: AtomicU8::new(DIR_NONE),
            flight_steps: AtomicI32::new(0),
            target_delta_x: AtomicCell::new(0.0),
            target_delta_y: AtomicCell::new(0.0),
            target_delta_z: AtomicCell::new(0.0),
            last_deflected_by: LastDeflectedBy::new(),
        }
    }

    /// Vanilla `ShulkerBullet.canHitEntity`: `Projectile.canHitEntity` without entities that
    /// pass through blocks, so bullets never hit each other.
    fn can_hit_entity(&self, world: &World, other: &dyn EntityBase) -> bool {
        let other_entity = other.get_entity();
        other_entity.entity_id != self.entity.entity_id
            && other_entity.is_alive()
            && !other.is_spectator()
            && is_pickable(other)
            && self.owner.can_hit(world, other_entity)
            && !other_entity.no_physics.load(Ordering::Relaxed)
    }

    /// Vanilla `ProjectileUtil.getHitResultOnMoveVector`: the first block on the way, unless an
    /// entity comes before it.
    fn hit_result_on_move_vector(&self, world: &World) -> Option<ProjectileHit> {
        let entity = &self.entity;
        let from = entity.pos.load();
        let delta = entity.velocity.load();
        let block_hit = world.clip(&ClipContext::new(
            from,
            from + delta,
            ClipBlock::Collider,
            ClipFluid::None,
        ));
        let to = block_hit.location;
        let area = entity
            .bounding_box
            .load()
            .expand_towards(delta.x, delta.y, delta.z)
            .expand_all(1.0);
        let candidates = hit_candidates(world, &area)
            .into_iter()
            .filter(|candidate| self.can_hit_entity(world, candidate.as_ref()));
        let normal = delta.normalize() * -1.0;
        if let Some((target, hit_pos)) =
            entity_hit_result(candidates, from, to, compute_margin(entity))
        {
            return Some(ProjectileHit::Entity {
                entity: target,
                hit_pos,
                normal,
            });
        }
        (!block_hit.is_miss).then_some(ProjectileHit::Block {
            pos: block_hit.block_pos,
            face: block_hit.direction,
            hit_pos: block_hit.location,
            normal,
        })
    }

    /// Vanilla `Projectile.hitTargetOrDeflectSelf`: an entity may deflect the bullet, and a
    /// block that turned to air since the clip is no hit.
    fn hit_target_or_deflect_self(&self, caller: &dyn EntityBase, hit: ProjectileHit) {
        match &hit {
            ProjectileHit::Entity { entity: target, .. } => {
                if self
                    .last_deflected_by
                    .hit_target_or_deflect_self(caller, target.as_ref())
                {
                    return;
                }
            }
            ProjectileHit::Block { pos, .. } => {
                if self.entity.world.load().get_block_state(pos).is_air() {
                    return;
                }
            }
        }
        caller.on_hit(hit);
    }

    /// Vanilla `ShulkerBullet.onHitEntity`: any target can be hurt, only living ones levitate.
    fn on_hit_entity(&self, target: &dyn EntityBase, owner: Option<&dyn EntityBase>) {
        // Vanilla `mobProjectile(this, livingOwner)` credits only a living owner.
        let cause = owner.filter(|owner| owner.get_living_entity().is_some());
        if !target.damage_with_context(
            target,
            HIT_DAMAGE,
            DamageType::MOB_PROJECTILE,
            None,
            Some(self),
            cause,
        ) {
            return;
        }
        crate::enchantment::EnchantmentHelper::do_post_attack_effects(
            target,
            DamageType::MOB_PROJECTILE,
            cause,
            Some(self),
        );
        if let Some(living) = target.get_living_entity() {
            living.add_effect_with_source(
                Effect {
                    effect_type: &StatusEffect::LEVITATION,
                    duration: LEVITATION_TICKS,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                },
                Some(owner.unwrap_or(self)),
            );
        }
    }

    /// Vanilla `ShulkerBullet.destroy`.
    fn destroy(&self) {
        let entity = &self.entity;
        entity.remove();
        entity.world.load().emit_game_event_with_context(
            GameEvent::EntityDamage.name(),
            entity.pos.load(),
            &GameEventContext {
                source_entity: Some(self),
            },
        );
    }

    /// The steering half of vanilla `ShulkerBullet.tick`: a new direction once the flight steps
    /// run out, the block ahead can be stood on or the target's row is reached.
    fn steer(&self, target: &Entity) {
        let steps = self.flight_steps.load(Ordering::Relaxed);
        if steps > 0 {
            self.flight_steps.store(steps - 1, Ordering::Relaxed);
            if steps == 1 {
                let axis = dir_axis(self.current_dir.load(Ordering::Relaxed));
                self.select_next_dir(axis, Some(target));
            }
        }

        let dir = self.current_dir.load(Ordering::Relaxed);
        if dir == DIR_NONE {
            return;
        }
        let current = self.entity.block_pos.load();
        let axis = dir_axis(dir);
        let ahead = BlockPos::new(
            current.0.x + dir_step_x_s(dir),
            current.0.y + dir_step_y_s(dir),
            current.0.z + dir_step_z_s(dir),
        );
        // Vanilla `loadedAndEntityCanStandOn`: a full top face of the collision shape.
        let blocked = self
            .entity
            .world
            .load()
            .get_block_state_id_if_loaded(&ahead)
            .is_some_and(|state| is_face_full(state, ahead.0, pumpkin_data::BlockDirection::Up));
        let target_pos = target.block_pos.load();
        let reached = match axis {
            0 => current.0.x == target_pos.0.x,
            1 => current.0.y == target_pos.0.y,
            _ => current.0.z == target_pos.0.z,
        };
        if blocked || reached {
            self.select_next_dir(axis, Some(target));
        }
    }
}

impl EntityBase for ShulkerBulletEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.owner)
    }

    /// Vanilla `ShulkerBullet.isPickable`: bullets can be shot or punched.
    fn can_hit(&self) -> bool {
        true
    }

    fn sound_source(&self) -> SoundCategory {
        SoundCategory::Hostile
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.target.write_nbt(nbt, "Target");
        let dir = self.current_dir.load(Ordering::Relaxed);
        if dir != DIR_NONE {
            nbt.put_byte("Dir", dir as i8);
        }
        nbt.put_int("Steps", self.flight_steps.load(Ordering::Relaxed));
        nbt.put_double("TXD", self.target_delta_x.load());
        nbt.put_double("TYD", self.target_delta_y.load());
        nbt.put_double("TZD", self.target_delta_z.load());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(steps) = nbt.get_int("Steps") {
            self.flight_steps.store(steps, Ordering::Relaxed);
        }
        for (key, delta) in [
            ("TXD", &self.target_delta_x),
            ("TYD", &self.target_delta_y),
            ("TZD", &self.target_delta_z),
        ] {
            if let Some(value) = nbt.get_double(key) {
                delta.store(value);
            }
        }
        // `Direction.LEGACY_ID_CODEC`: `from3DDataValue`, whose order the `DIR_` ids follow.
        if let Some(dir) = nbt.get_byte("Dir") {
            self.current_dir
                .store((i32::from(dir) % 6).unsigned_abs() as u8, Ordering::Relaxed);
        }
        self.target.read_nbt(nbt, "Target");
    }

    fn remove_custom_nbt(&self, removed: &[Box<str>]) {
        for key in removed {
            match &**key {
                "Target" => self.target.clear(),
                "Dir" => self.current_dir.store(DIR_NONE, Ordering::Relaxed),
                "Steps" => self.flight_steps.store(0, Ordering::Relaxed),
                "TXD" => self.target_delta_x.store(0.0),
                "TYD" => self.target_delta_y.store(0.0),
                "TZD" => self.target_delta_z.store(0.0),
                _ => {}
            }
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `ShulkerBullet.hurtServer`: any hit destroys the bullet.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = &self.entity;
        if entity.is_removed() {
            return false;
        }
        self.play_entity_sound(Sound::EntityShulkerBulletHurt, 1.0, 1.0);
        entity.world.load().spawn_particle(
            entity.pos.load(),
            Vector3::new(0.2, 0.2, 0.2),
            0.0,
            15,
            Particle::Crit,
        );
        self.destroy();
        true
    }

    /// Vanilla `ShulkerBullet.onHit`: a hit projectile is turned the way the bullet flew, a block
    /// hit bursts, and the bullet is gone either way.
    fn on_hit(&self, hit: ProjectileHit) {
        let entity = &self.entity;
        let world = entity.world.load();
        match hit {
            ProjectileHit::Entity { entity: target, .. } => {
                let owner = self.owner.get(&world);
                redirect_hit_projectile(
                    target.as_ref(),
                    ProjectileDeflectionType::MomentumDeflect,
                    Some(self),
                    owner.as_deref(),
                );
                self.on_hit_entity(target.as_ref(), owner.as_deref());
            }
            ProjectileHit::Block { pos, hit_pos, .. } => {
                let (block, state) = world.get_block_and_state(&pos);
                if let Some(server) = world.server.upgrade() {
                    world
                        .block_registry
                        .on_projectile_hit(block, &world, self, &pos, state, &hit_pos, &server);
                }
                world.spawn_particle(
                    entity.pos.load(),
                    Vector3::new(0.2, 0.2, 0.2),
                    0.0,
                    2,
                    Particle::Explosion,
                );
                self.play_entity_sound(Sound::EntityShulkerBulletHit, 1.0, 1.0);
            }
        }
        self.destroy();
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = &self.entity;
        let world = entity.world.load();
        // Vanilla `ShulkerBullet.checkDespawn`.
        if world.level_info.load().difficulty == Difficulty::Peaceful {
            entity.remove();
            return;
        }
        self.owner.check_left_owner(entity);
        // `Entity.baseTick` `computeSpeed`, the known speed a momentum deflection reads.
        entity.update_last_pos();

        let target = self.target.get(&world);
        if target.is_none() {
            self.target.clear();
        }
        let tracking = target.as_ref().is_some_and(|target| {
            target.get_entity().is_alive()
                && !(target.get_player().is_some() && target.is_spectator())
        });
        let mut movement = entity.velocity.load();
        if tracking {
            let tdx = (self.target_delta_x.load() * 1.025).clamp(-1.0, 1.0);
            let tdy = (self.target_delta_y.load() * 1.025).clamp(-1.0, 1.0);
            let tdz = (self.target_delta_z.load() * 1.025).clamp(-1.0, 1.0);
            self.target_delta_x.store(tdx);
            self.target_delta_y.store(tdy);
            self.target_delta_z.store(tdz);
            movement = movement
                + Vector3::new(
                    (tdx - movement.x) * 0.2,
                    (tdy - movement.y) * 0.2,
                    (tdz - movement.z) * 0.2,
                );
        } else if !entity.has_no_gravity() {
            movement.y -= GRAVITY;
        }
        entity.velocity.store(movement);

        // Vanilla finds the hit first, then makes the whole move whatever it hit.
        let hit = self.hit_result_on_move_vector(&world);
        entity.set_pos(entity.pos.load() + movement);
        if let Some(hit) = hit {
            self.hit_target_or_deflect_self(caller, hit);
        }
        rotate_towards_movement(entity, ROTATION_SPEED);

        if entity.is_removed() {
            return;
        }
        // Vanilla steers towards any target still in the world, alive or not.
        if let Some(target) = target {
            self.steer(target.get_entity());
        }
    }
}
