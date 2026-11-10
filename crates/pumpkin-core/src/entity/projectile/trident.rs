use crate::enchantment::helper::EnchantmentHelper;
use crate::entity::projectile::owner::ProjectileOwner;
use crate::entity::projectile_deflection::{LastDeflectedBy, redirect_projectile};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component_impl::{EnchantmentGlintOverrideImpl, EnchantmentsImpl};
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    entity::{Entity, EntityBase, living::LivingEntity, player::Player},
    server::Server,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_protocol::IdOr;
use pumpkin_protocol::java::client::play::CSoundEffect;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::arrow::ArrowPickup;
use super::{
    ProjectileHit, arrow_rest_position, calculate_ray_intersection, compute_margin,
    entity_hit_result,
};

pub struct TridentEntity {
    pub entity: Entity,
    pub owner: ProjectileOwner,
    pub item_stack: Arc<Mutex<ItemStack>>,
    pub pickup: AtomicCell<ArrowPickup>,
    pub in_ground: AtomicBool,
    pub in_ground_time: AtomicU32,
    pub life: AtomicU32,
    pub shake_time: AtomicU8,
    pub has_hit: AtomicBool,
    pub last_deflected_by: LastDeflectedBy,
    /// Vanilla `dealtDamage`: a trident hurts at most one entity.
    pub dealt_damage: AtomicBool,
    /// Vanilla `isNoPhysics`: set while a Loyalty trident flies back through blocks.
    no_physics: AtomicBool,
    /// Vanilla `ID_LOYALTY`, the return acceleration from the item's enchantments.
    loyalty: AtomicU8,
    /// Vanilla `clientSideReturnTridentTickCount`, only used to play the return sound once.
    return_ticks: AtomicU32,
    pub last_block_pos: Arc<std::sync::RwLock<Option<BlockPos>>>,
}

/// The power of the `ProjectileDeflection.REVERSE` vanilla `ThrownTrident.onHitEntity` bounces
/// the trident back with.
const TRIDENT_HIT_BOUNCE: Vector3<f64> = Vector3::new(0.02, 0.2, 0.02);

impl TridentEntity {
    const BASE_DAMAGE: f64 = 8.0;
    const AIR_INERTIA: f64 = 0.99;
    const WATER_INERTIA: f64 = 0.99;
    const GRAVITY: f64 = 0.05;
    const DESPAWN_TIME: u32 = 1200;

    pub fn new(entity: Entity, owner: Option<&Entity>) -> Self {
        // Vanilla `ThrownTrident.defineSynchedData`.
        crate::entity::projectile::arrow::define_abstract_arrow_synched_data(&entity);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::trident::ID_LOYALTY, 0i8);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::trident::ID_FOIL, false);
        Self {
            entity,
            owner: ProjectileOwner::new(owner),
            item_stack: Arc::new(Mutex::new(ItemStack::new(1, &Item::TRIDENT))),
            pickup: AtomicCell::new(ArrowPickup::Disallowed),
            in_ground: AtomicBool::new(false),
            in_ground_time: AtomicU32::new(0),
            life: AtomicU32::new(0),
            shake_time: AtomicU8::new(0),
            has_hit: AtomicBool::new(false),
            last_deflected_by: LastDeflectedBy::new(),
            dealt_damage: AtomicBool::new(false),
            no_physics: AtomicBool::new(false),
            loyalty: AtomicU8::new(0),
            return_ticks: AtomicU32::new(0),
            last_block_pos: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        item_stack: ItemStack,
        pickup: ArrowPickup,
    ) -> Self {
        let mut owner_pos = shooter.pos.load();
        owner_pos.y = owner_pos.y + f64::from(shooter.entity_dimension.load().eye_height) - 0.1;
        entity.set_pos(owner_pos);
        entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));

        let loyalty = Self::loyalty_from_item(&item_stack);
        Self {
            entity,
            owner: ProjectileOwner::new(Some(shooter)),
            item_stack: Arc::new(Mutex::new(item_stack)),
            pickup: AtomicCell::new(pickup),
            in_ground: AtomicBool::new(false),
            in_ground_time: AtomicU32::new(0),
            life: AtomicU32::new(0),
            shake_time: AtomicU8::new(0),
            has_hit: AtomicBool::new(false),
            last_deflected_by: LastDeflectedBy::new(),
            dealt_damage: AtomicBool::new(false),
            no_physics: AtomicBool::new(false),
            loyalty: AtomicU8::new(loyalty),
            return_ticks: AtomicU32::new(0),
            last_block_pos: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// Vanilla `ThrownTrident.getWeaponItem`: the trident item it was thrown as.
    fn weapon_item(&self) -> ItemStack {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `getLoyaltyFromItem`: the item's `trident_return_acceleration`, kept in 0..=127.
    fn loyalty_from_item(stack: &ItemStack) -> u8 {
        let mut acceleration = 0.0f32;
        if let Some(enchantments) = stack.get_data_component::<EnchantmentsImpl>() {
            for (enchantment, level) in enchantments.enchantment.iter() {
                for effect in enchantment.effects.trident_return_acceleration {
                    acceleration = effect.effect.process(*level, acceleration);
                }
            }
        }
        acceleration.clamp(0.0, 127.0) as u8
    }

    /// Vanilla `ThrownTrident.tick`'s Loyalty part. Returns `false` when the trident is gone.
    fn tick_loyalty(&self, world: &crate::world::World) -> bool {
        let entity = &self.entity;
        if self.in_ground_time.load(Ordering::Relaxed) > 4 {
            self.dealt_damage.store(true, Ordering::Relaxed);
        }
        let loyalty = self.loyalty.load(Ordering::Relaxed);
        if loyalty == 0
            || !(self.dealt_damage.load(Ordering::Relaxed)
                || self.no_physics.load(Ordering::Relaxed))
        {
            return true;
        }
        let Some(owner) = self.owner.get(world) else {
            return true;
        };
        // Vanilla `isAcceptibleReturnOwner`.
        let acceptable = owner
            .get_living_entity()
            .map_or_else(|| owner.get_entity().is_alive(), LivingEntity::is_alive)
            && !(owner.get_player().is_some() && owner.is_spectator());
        if !acceptable {
            if self.pickup.load() == ArrowPickup::Allowed {
                let stack = self
                    .item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                entity.spawn_at_location(stack);
            }
            entity.remove();
            return false;
        }
        let owner_entity = owner.get_entity();
        let owner_eye = owner_entity.get_eye_pos();
        let pos = entity.pos.load();
        if owner.get_player().is_none()
            && pos.squared_distance_to_vec(&owner_eye).sqrt()
                < f64::from(owner_entity.width()) + 1.0
        {
            entity.remove();
            return false;
        }

        self.set_no_physics(true);
        let to_owner = owner_eye.sub(&pos);
        let loyalty = f64::from(loyalty);
        entity.set_pos(Vector3::new(
            pos.x,
            to_owner.y.mul_add(0.015 * loyalty, pos.y),
            pos.z,
        ));
        let acceleration = to_owner.normalize() * (0.05 * loyalty);
        entity
            .velocity
            .store(entity.velocity.load() * 0.95 + acceleration);
        if self.return_ticks.fetch_add(1, Ordering::Relaxed) == 0 {
            world.play_sound_fine(
                Sound::ItemTridentReturn,
                SoundCategory::Neutral,
                &entity.pos.load(),
                10.0,
                1.0,
            );
        }
        true
    }

    fn set_no_physics(&self, no_physics: bool) {
        if self.no_physics.swap(no_physics, Ordering::Relaxed) == no_physics {
            return;
        }
        if no_physics {
            self.in_ground.store(false, Ordering::Relaxed);
        }
        // Vanilla `AbstractArrow.FLAG_NOPHYSICS`.
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::trident::ID_FLAGS,
            if no_physics { 2i8 } else { 0i8 },
        );
    }

    /// Applies projectile-spawned enchantment effects matching vanilla `Projectile::applyOnProjectileSpawned`.
    pub fn apply_on_projectile_spawned(&self, pickup_item_stack: &ItemStack) {
        super::apply_on_projectile_spawned(self.get_entity(), pickup_item_stack, None, None);
    }

    pub fn set_velocity_from_rotation(
        &self,
        pitch: f32,
        yaw: f32,
        roll: f32,
        speed: f32,
        divergence: f32,
    ) {
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

    fn should_skip_collision(&self, self_ent: &Entity, other: &Arc<dyn EntityBase>) -> bool {
        let other_ent = other.get_entity();

        // Don't collide with self
        if other_ent.entity_id == self_ent.entity_id {
            return true;
        }

        if other.is_spectator() || !self.owner.can_hit(&self_ent.world.load(), other_ent) {
            return true;
        }

        !super::can_be_hit_by_projectile(other.as_ref())
    }
}

impl EntityBase for TridentEntity {
    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.owner)
    }

    #[expect(clippy::too_many_lines, reason = "mirrors vanilla AbstractArrow.tick")]
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if !self.tick_loyalty(&world) {
            return;
        }
        self.owner.check_left_owner(entity);

        // Handle shake time
        let shake = self.shake_time.load(Ordering::Relaxed);
        if shake > 0 {
            self.shake_time.store(shake - 1, Ordering::Relaxed);
        }

        let no_physics = self.no_physics.load(Ordering::Relaxed);
        if self.in_ground.load(Ordering::Relaxed) && !no_physics {
            self.in_ground_time.fetch_add(1, Ordering::Relaxed);
            // Vanilla `ThrownTrident.tickDespawn`: a Loyalty trident its owner can pick up stays.
            if self.pickup.load() != ArrowPickup::Allowed
                || self.loyalty.load(Ordering::Relaxed) == 0
            {
                let life = self.life.fetch_add(1, Ordering::Relaxed);
                if life + 1 >= Self::DESPAWN_TIME {
                    entity.remove();
                }
            }
            return;
        }
        self.in_ground_time.store(0, Ordering::Relaxed);

        // Vanilla `AbstractArrow.tick`: water slows the trident before it moves, air after, and
        // gravity comes last.
        let start_pos = entity.pos.load();
        let in_water = entity.touching_water.load(Ordering::Relaxed);
        let mut velocity = entity.velocity.load();
        if in_water {
            velocity = velocity * Self::WATER_INERTIA;
        }

        let len = velocity.horizontal_length();
        let yaw = if no_physics {
            (-velocity.x).atan2(-velocity.z)
        } else {
            velocity.x.atan2(velocity.z)
        };
        entity.set_yaw_pitch(
            yaw as f32 * 57.295_776,
            velocity.y.atan2(len) as f32 * 57.295_776,
        );

        let new_pos = start_pos.add(&velocity);

        let after_move = |trident: &Self| {
            let mut velocity = trident.entity.velocity.load();
            if !in_water {
                velocity = velocity * Self::AIR_INERTIA;
            }
            // Vanilla `Entity.applyGravity`, which `NoGravity` turns off.
            if !no_physics
                && !trident.in_ground.load(Ordering::Relaxed)
                && !trident.entity.has_no_gravity()
            {
                velocity.y -= Self::GRAVITY;
            }
            trident.entity.velocity.store(velocity);
        };
        if no_physics {
            entity.set_pos(new_pos);
            entity.velocity.store(velocity);
            after_move(self);
            return;
        }
        entity.velocity.store(velocity);

        // Check for collisions using raycasting
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

        // Block collisions
        let (block_cols, block_positions) =
            world.get_block_collisions(search_box, self.get_entity());
        for (idx, bb) in block_cols.iter().enumerate() {
            if let Some(t) = calculate_ray_intersection(&start_pos, &velocity, bb)
                && t < closest_t
            {
                closest_t = t;

                // Map back to block pos
                for (len, pos) in &block_positions {
                    if idx < *len {
                        let hit_pos = start_pos.add(&velocity.multiply(t, t, t));
                        hit = Some(ProjectileHit::Block {
                            pos: *pos,
                            face: get_hit_face(hit_pos, *pos),
                            hit_pos,
                            normal: velocity.normalize().multiply(-1.0, -1.0, -1.0),
                        });
                        break;
                    }
                }
            }
        }

        // Vanilla `ThrownTrident.findHitEntities`: the nearest entity up to the block hit, until
        // the trident has dealt damage, and the trident moves to where it hits.
        let block_to = start_pos + velocity * closest_t;
        let entity_hit = if self.dealt_damage.load(Ordering::Relaxed) {
            None
        } else {
            let candidates = super::hit_candidates(&world, &search_box)
                .into_iter()
                .filter(|cand| !self.should_skip_collision(entity, cand));
            entity_hit_result(candidates, start_pos, block_to, compute_margin(entity))
        };
        if let Some((target, hit_pos)) = entity_hit {
            entity.set_pos(hit_pos);
            hit = Some(ProjectileHit::Entity {
                entity: target,
                hit_pos,
                normal: velocity.normalize().multiply(-1.0, -1.0, -1.0),
            });
        } else {
            entity.set_pos(block_to);
        }

        // Handle hit
        if let Some(h) = hit
            && !matches!(&h, ProjectileHit::Entity { entity: target, .. }
                if self.last_deflected_by.hit_target_or_deflect_self(caller, target.as_ref()))
            && !self.has_hit.swap(true, Ordering::SeqCst)
        {
            if let ProjectileHit::Entity { entity: target, .. } = &h {
                redirect_projectile(caller, target.as_ref());
            }
            caller.on_hit(h);
            // Vanilla `AbstractArrow.stepMoveAndHit` sets `needsSync` after a hit.
            entity.velocity_dirty.store(true, Ordering::Relaxed);
        }
        after_move(self);
    }

    fn write_custom_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        let mut item = pumpkin_nbt::compound::NbtCompound::new();
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write_item_stack(&mut item);
        nbt.put_compound("item", item);
        nbt.put_bool("inGround", self.in_ground.load(Ordering::Relaxed));
        nbt.put_int("life", self.life.load(Ordering::Relaxed) as i32);
        nbt.put_byte("shake", self.shake_time.load(Ordering::Relaxed) as i8);
        nbt.put_byte("pickup", self.pickup.load().to_byte() as i8);
        nbt.put_bool("DealtDamage", self.dealt_damage.load(Ordering::Relaxed));
    }

    fn read_custom_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        if let Some(stack) = nbt
            .get_compound("item")
            .and_then(ItemStack::read_item_stack)
        {
            self.loyalty
                .store(Self::loyalty_from_item(&stack), Ordering::Relaxed);
            *self
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        }
        if let Some(in_ground) = nbt.get_bool("inGround") {
            self.in_ground.store(in_ground, Ordering::Relaxed);
        }
        if let Some(life) = nbt.get_int("life") {
            self.life.store(life.max(0) as u32, Ordering::Relaxed);
        }
        if let Some(shake) = nbt.get_byte("shake") {
            self.shake_time.store(shake.max(0) as u8, Ordering::Relaxed);
        }
        // Vanilla falls back to `DISALLOWED` when the tag is missing.
        self.pickup.store(ArrowPickup::from_byte(
            nbt.get_byte("pickup").unwrap_or(0) as u8
        ));
        self.dealt_damage.store(
            nbt.get_bool("DealtDamage").unwrap_or(false),
            Ordering::Relaxed,
        );
    }

    fn init_data_tracker(&self) {
        let stack = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let foil = stack
            .get_data_component::<EnchantmentGlintOverrideImpl>()
            .map_or_else(
                || {
                    stack
                        .get_data_component::<EnchantmentsImpl>()
                        .is_some_and(|e| !e.enchantment.is_empty())
                },
                |glint| glint.glint,
            );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::trident::ID_LOYALTY,
            self.loyalty.load(Ordering::Relaxed) as i8,
        );
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::trident::ID_FOIL, foil);
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

    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load();

        match hit {
            ProjectileHit::Block { pos, hit_pos, .. } => {
                self.in_ground.store(true, Ordering::Relaxed);
                self.shake_time.store(7, Ordering::Relaxed);
                *self
                    .last_block_pos
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pos);

                let block = world.get_block(&pos);
                let state = world.get_block_state(&pos);
                if let Some(server) = world.server.upgrade() {
                    world
                        .block_registry
                        .on_projectile_hit(block, &world, self, &pos, state, &hit_pos, &server);
                }
                // Vanilla `ThrownTrident.hitBlockEnchantmentEffects`.
                let owner = self.owner.get(&world);
                EnchantmentHelper::on_hit_block(
                    &self.weapon_item(),
                    owner.as_deref(),
                    self,
                    pos.clamp_location_within(hit_pos),
                    state,
                );

                // Stop the trident
                entity.set_pos(arrow_rest_position(hit_pos, entity.velocity.load()));
                entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));

                // Play sound
                let sound_packet = CSoundEffect::new(
                    IdOr::Id(Sound::ItemTridentHitGround as u16),
                    SoundCategory::Neutral,
                    &hit_pos,
                    1.0,
                    1.0,
                    0,
                );
                let chunk_pos = entity.chunk_pos.load();
                world.broadcast_to_chunk(chunk_pos, &sound_packet);
            }
            ProjectileHit::Entity {
                entity: target,
                hit_pos,
                ..
            } => {
                let mut damage = Self::BASE_DAMAGE;

                // Apply Impaling enchantment extra damage
                if let Some(enchantments) = self
                    .item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
                {
                    for (enchantment, level) in enchantments.enchantment.iter() {
                        if **enchantment == pumpkin_data::Enchantment::IMPALING {
                            let in_water =
                                target.get_entity().touching_water.load(Ordering::Relaxed);
                            if in_water {
                                damage += 1.25 * f64::from(*level);
                            }
                        }
                    }
                }

                self.dealt_damage.store(true, Ordering::Relaxed);
                let owner = self.owner.get(&world);
                // Like vanilla, an ownerless trident is its own cause.
                let cause = owner.as_deref().unwrap_or(self);
                let was_hurt = target.damage_with_context(
                    target.as_ref(),
                    damage as f32,
                    DamageType::TRIDENT,
                    Some(hit_pos),
                    Some(self),
                    Some(cause),
                );
                if was_hurt {
                    EnchantmentHelper::do_post_attack_effects_with_item_source(
                        target.as_ref(),
                        DamageType::TRIDENT,
                        Some(cause),
                        Some(self),
                        Some(&self.weapon_item()),
                    );
                }

                // Play hit sound
                let sound_packet = CSoundEffect::new(
                    IdOr::Id(Sound::ItemTridentHit as u16),
                    SoundCategory::Neutral,
                    &hit_pos,
                    1.0,
                    1.0,
                    0,
                );
                let chunk_pos = entity.chunk_pos.load();
                world.broadcast_to_chunk(chunk_pos, &sound_packet);

                // Vanilla bounces it back with `ProjectileDeflection.REVERSE`.
                crate::entity::projectile_deflection::reverse(self, TRIDENT_HIT_BOUNCE);
                self.has_hit.store(false, Ordering::Relaxed); // Let it hit the ground
            }
        }
    }

    fn receives_player_touch(&self) -> bool {
        true
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        // Vanilla `ThrownTrident.playerTouch`: only its owner, or anyone for an ownerless one.
        let world = self.entity.world.load();
        let owned = self.owner.owned_by(player.get_entity());
        if !owned && self.owner.get(&world).is_some() {
            return;
        }
        let no_physics = self.no_physics.load(Ordering::Relaxed);
        if !(self.in_ground.load(Ordering::Relaxed) || no_physics)
            || self.shake_time.load(Ordering::Relaxed) > 0
        {
            return;
        }

        if player.living_entity.health.load() <= 0.0 {
            return;
        }

        // Vanilla `tryPickup`: the pickup rule, or a returning trident reaching its owner.
        let allowed = match self.pickup.load() {
            ArrowPickup::Disallowed => false,
            ArrowPickup::Allowed => true,
            ArrowPickup::CreativeOnly => player.is_creative(),
        };
        if !(allowed || no_physics && owned) {
            return;
        }

        let mut stack = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let creative_only = self.pickup.load() == ArrowPickup::CreativeOnly && player.is_creative();
        if creative_only || player.inventory.insert_stack_anywhere(&mut stack) {
            player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::PickedUp,
                stack.item.id as i32,
                1,
            );
            player.living_entity.pickup(&self.entity, 1);
            self.get_entity().remove();
        }
    }
}

/// Get the face of the block that was hit
fn get_hit_face(hit_pos: Vector3<f64>, block_pos: BlockPos) -> pumpkin_data::BlockDirection {
    let local = hit_pos.sub(&block_pos.0.to_f64());
    let eps = 1.0e-4;

    if local.x <= eps {
        pumpkin_data::BlockDirection::West
    } else if local.x >= 1.0 - eps {
        pumpkin_data::BlockDirection::East
    } else if local.y <= eps {
        pumpkin_data::BlockDirection::Down
    } else if local.y >= 1.0 - eps {
        pumpkin_data::BlockDirection::Up
    } else if local.z <= eps {
        pumpkin_data::BlockDirection::North
    } else {
        pumpkin_data::BlockDirection::South
    }
}
