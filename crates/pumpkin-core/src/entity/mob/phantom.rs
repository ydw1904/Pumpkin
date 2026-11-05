use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{
    approach, approach_degrees, cos, degrees_difference_abs, sin, wrap_degrees,
};
use pumpkin_world::chunk::ChunkHeightmapType;

use crate::entity::ai::brain::memory::codec::{block_pos_from_tag, block_pos_to_tag};
use crate::entity::ai::control::{Control, LookControlTrait, MoveControlTrait};
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::cat::CatEntity;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

/// Vanilla `Phantom.TICKS_PER_FLAP`, `Mth.ceil(24.166098F)`.
const TICKS_PER_FLAP: i32 = 25;
/// `ATTACK_DAMAGE` is 6 plus the phantom's size.
const BASE_ATTACK_DAMAGE: f64 = 6.0;
const MAX_SIZE: i32 = 64;
const DEG_TO_RAD: f32 = PI / 180.0;

pub struct PhantomEntity {
    pub mob_entity: MobEntity,
    move_target_point: AtomicCell<Vector3<f64>>,
    anchor_point: AtomicCell<Option<BlockPos>>,
    /// Vanilla `AttackPhase.SWOOP`; `CIRCLE` otherwise.
    swooping: AtomicBool,
    size: AtomicI32,
}

impl PhantomEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Phantom.defineSynchedData`.
        entity
            .synched_data
            .define(tracked_data::phantom::ID_SIZE, pumpkin_protocol::VarInt(0));
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(PhantomMoveControl { speed: 0.1 });
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(PhantomLookControl);
        mob_entity
            .living_entity
            .set_attribute_base(&Attributes::ATTACK_DAMAGE, BASE_ATTACK_DAMAGE);
        let mob_arc = Arc::new(Self {
            mob_entity,
            move_target_point: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            anchor_point: AtomicCell::new(None),
            swooping: AtomicBool::new(false),
            size: AtomicI32::new(0),
        });
        let phantom = Arc::downgrade(&mob_arc);

        let goal_selector = &mob_arc.mob_entity.goals_selector;
        goal_selector.add_goal(
            1,
            Box::new(PhantomAttackStrategyGoal {
                phantom: phantom.clone(),
                next_sweep_tick: 0,
            }),
        );
        goal_selector.add_goal(
            2,
            Box::new(PhantomSweepAttackGoal {
                phantom: phantom.clone(),
                is_scared_of_cat: false,
                cat_search_tick: 0,
            }),
        );
        goal_selector.add_goal(
            3,
            Box::new(PhantomCircleAroundAnchorGoal {
                phantom: phantom.clone(),
                angle: 0.0,
                distance: 0.0,
                height: 0.0,
                clockwise: 0.0,
            }),
        );
        mob_arc.mob_entity.target_selector.add_goal(
            1,
            Box::new(PhantomAttackPlayerTargetGoal {
                phantom,
                next_scan_tick: to_goal_ticks(20),
            }),
        );

        mob_arc
    }

    #[must_use]
    pub fn get_phantom_size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    pub fn set_phantom_size(&self, size: i32) {
        let size = size.clamp(0, MAX_SIZE);
        self.size.store(size, Ordering::Relaxed);
        let living = &self.mob_entity.living_entity;
        living
            .entity
            .set_synced_data(tracked_data::phantom::ID_SIZE, size);
        // Vanilla `getDefaultDimensions` scales the type's size by `1 + 0.15 * size`.
        let entity = &living.entity;
        let scale = 0.15f32.mul_add(size as f32, 1.0);
        entity.set_default_dimensions(EntityDimensions::new(
            entity.entity_type.dimension[0] * scale,
            entity.entity_type.dimension[1] * scale,
            entity.entity_type.eye_height * scale,
        ));
        living.set_attribute_base(
            &Attributes::ATTACK_DAMAGE,
            BASE_ATTACK_DAMAGE + f64::from(size),
        );
    }

    const fn entity(&self) -> &Entity {
        &self.mob_entity.living_entity.entity
    }

    /// Vanilla `PhantomMoveTargetGoal.touchingTarget`.
    fn touching_target(&self) -> bool {
        self.move_target_point
            .load()
            .squared_distance_to_vec(&self.entity().pos.load())
            < 4.0
    }

    /// Vanilla `canAttack(level, target, TargetingConditions.DEFAULT)`.
    fn can_attack_target(&self, target: &dyn EntityBase) -> bool {
        TargetPredicate::create_attackable().test(&self.entity().world.load(), Some(self), target)
    }
}

/// Vanilla `PhantomMoveControl`: steer straight at `moveTargetPoint`, speeding up on straight
/// lines and slowing down in turns.
struct PhantomMoveControl {
    speed: f32,
}

impl Control for PhantomMoveControl {}

impl MoveControlTrait for PhantomMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(phantom) = mob.cast_any().downcast_ref::<PhantomEntity>() else {
            return;
        };
        let entity = phantom.entity();
        if entity.horizontal_collision.load(Ordering::SeqCst) {
            entity.yaw.store(entity.yaw.load() + 180.0);
            self.speed = 0.1;
        }

        let target = phantom.move_target_point.load();
        let pos = entity.pos.load();
        let mut tdx = target.x - pos.x;
        let tdy = target.y - pos.y;
        let mut tdz = target.z - pos.z;
        let sd = tdx.hypot(tdz);
        if sd.abs() <= f64::from(1.0E-5f32) {
            return;
        }
        let y_relative_scale = 1.0 - (tdy * f64::from(0.7f32)).abs() / sd;
        tdx *= y_relative_scale;
        tdz *= y_relative_scale;
        let sd = tdx.hypot(tdz);
        let sd2 = (tdx * tdx + tdz * tdz + tdy * tdy).sqrt();
        let prev = entity.yaw.load();
        let angle = tdz.atan2(tdx) as f32;
        let a = wrap_degrees(prev + 90.0);
        let b = wrap_degrees(angle * (180.0 / PI));
        let yaw = approach_degrees(a, b, 4.0) - 90.0;
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
        self.speed = if degrees_difference_abs(prev, yaw) < 3.0 {
            approach(self.speed, 1.8, 0.005 * (1.8 / self.speed))
        } else {
            approach(self.speed, 0.2, 0.025)
        };

        let x_rot_d = (-((-tdy).atan2(sd) * 180.0 / f64::from(PI))) as f32;
        entity.pitch.store(x_rot_d);
        let move_angle = (yaw + 90.0) * DEG_TO_RAD;
        let txd = f64::from(self.speed * cos(move_angle)) * (tdx / sd2).abs();
        let tzd = f64::from(self.speed * sin(move_angle)) * (tdz / sd2).abs();
        let tyd = f64::from(self.speed * sin(x_rot_d * DEG_TO_RAD)) * (tdy / sd2).abs();
        let movement = entity.velocity.load();
        entity
            .velocity
            .store(movement + (Vector3::new(txd, tyd, tzd) - movement) * 0.2);
    }
}

/// Vanilla `PhantomLookControl`: the move control sets the rotation, so looking does nothing.
struct PhantomLookControl;

impl Control for PhantomLookControl {}

impl LookControlTrait for PhantomLookControl {
    fn tick(&mut self, _mob: &dyn Mob) {}

    fn look_at_with_range(&mut self, _x: f64, _y: f64, _z: f64, _yaw: f32, _pitch: f32) {}
}

/// Vanilla `PhantomAttackPlayerTargetGoal`: every 3 seconds, target the highest attackable
/// player below.
struct PhantomAttackPlayerTargetGoal {
    phantom: Weak<PhantomEntity>,
    next_scan_tick: i32,
}

impl Goal for PhantomAttackPlayerTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.next_scan_tick > 0 {
            self.next_scan_tick -= 1;
            return false;
        }
        self.next_scan_tick = to_goal_ticks(60);
        let Some(phantom) = self.phantom.upgrade() else {
            return false;
        };
        let entity = phantom.entity();
        let world = entity.world.load();
        let area = entity.bounding_box.load().expand(16.0, 64.0, 16.0);
        let attack_targeting = TargetPredicate::create_attackable().set_base_max_distance(64.0);
        // `getNearbyPlayers` keeps the players whose position is inside the area.
        let mut players: Vec<_> = world
            .players
            .load()
            .iter()
            .filter(|player| {
                let pos = player.get_entity().pos.load();
                (area.min.x..area.max.x).contains(&pos.x)
                    && (area.min.y..area.max.y).contains(&pos.y)
                    && (area.min.z..area.max.z).contains(&pos.z)
                    && attack_targeting.test(&world, Some(mob), player.as_ref())
            })
            .cloned()
            .collect();
        players.sort_by(|a, b| {
            b.get_entity()
                .pos
                .load()
                .y
                .total_cmp(&a.get_entity().pos.load().y)
        });
        for player in players {
            if phantom.can_attack_target(player.as_ref()) {
                mob.set_mob_target(Some(player), TargetReason::ClosestPlayer);
                return true;
            }
        }
        false
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(phantom) = self.phantom.upgrade() else {
            return false;
        };
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| phantom.can_attack_target(target.as_ref()))
    }
}

/// Vanilla `PhantomAttackStrategyGoal`: circle high above the target, then swoop every 8 to 12
/// seconds.
struct PhantomAttackStrategyGoal {
    phantom: Weak<PhantomEntity>,
    next_sweep_tick: i32,
}

impl PhantomAttackStrategyGoal {
    fn set_anchor_above_target(phantom: &PhantomEntity) {
        if phantom.anchor_point.load().is_none() {
            return;
        }
        let Some(target) = phantom.mob_entity.get_target() else {
            return;
        };
        let world = phantom.entity().world.load();
        let mut anchor = target
            .get_entity()
            .block_pos
            .load()
            .up_height(20 + phantom.get_random().random_range(0..20));
        if anchor.0.y < world.sea_level {
            anchor = BlockPos::new(anchor.0.x, world.sea_level + 1, anchor.0.z);
        }
        phantom.anchor_point.store(Some(anchor));
    }
}

impl Goal for PhantomAttackStrategyGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(phantom) = self.phantom.upgrade() else {
            return false;
        };
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| phantom.can_attack_target(target.as_ref()))
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(phantom) = self.phantom.upgrade() else {
            return;
        };
        self.next_sweep_tick = self.get_tick_count(10);
        phantom.swooping.store(false, Ordering::Relaxed);
        Self::set_anchor_above_target(&phantom);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        let Some(phantom) = self.phantom.upgrade() else {
            return;
        };
        if let Some(anchor) = phantom.anchor_point.load() {
            let world = phantom.entity().world.load();
            // `getHeightmapPos` is the first free block above the heightmap's top block.
            let top = world.get_heightmap_height(
                ChunkHeightmapType::MotionBlocking,
                anchor.0.x,
                anchor.0.z,
            ) + 1;
            let height = 10 + phantom.get_random().random_range(0..20);
            phantom
                .anchor_point
                .store(Some(BlockPos::new(anchor.0.x, top + height, anchor.0.z)));
        }
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(phantom) = self.phantom.upgrade() else {
            return;
        };
        if phantom.swooping.load(Ordering::Relaxed) {
            return;
        }
        self.next_sweep_tick -= 1;
        if self.next_sweep_tick <= 0 {
            phantom.swooping.store(true, Ordering::Relaxed);
            Self::set_anchor_above_target(&phantom);
            let random = phantom.get_random();
            self.next_sweep_tick = self.get_tick_count((8 + random.random_range(0..4)) * 20);
            let entity = phantom.entity();
            if !entity.is_silent() {
                entity.world.load().play_sound_fine(
                    Sound::EntityPhantomSwoop,
                    SoundCategory::Hostile,
                    &entity.pos.load(),
                    10.0,
                    0.95 + random.random::<f32>() * 0.1,
                );
            }
        }
    }
}

/// Vanilla `PhantomSweepAttackGoal`: dive at the target and bite it, unless a cat is near.
struct PhantomSweepAttackGoal {
    phantom: Weak<PhantomEntity>,
    is_scared_of_cat: bool,
    cat_search_tick: i32,
}

impl Goal for PhantomSweepAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.phantom.upgrade().is_some_and(|phantom| {
            mob.get_mob_entity().get_target().is_some() && phantom.swooping.load(Ordering::Relaxed)
        })
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        if !target
            .get_living_entity()
            .is_some_and(crate::entity::living::LivingEntity::is_alive)
            || target
                .get_player()
                .is_some_and(|player| player.is_spectator() || player.is_creative())
            || !self.can_start(mob)
        {
            return false;
        }
        let entity = mob.get_entity();
        let tick_count = entity.tick_count.load(Ordering::Relaxed);
        if tick_count > self.cat_search_tick {
            self.cat_search_tick = tick_count + 20;
            let world = entity.world.load();
            let area = entity.bounding_box.load().expand_all(16.0);
            let mut cats_nearby = false;
            for cat in world.get_entities_at_box(&area) {
                if cat.get_entity().is_alive()
                    && let Some(cat) = cat.cast_any().downcast_ref::<CatEntity>()
                {
                    cats_nearby = true;
                    // Vanilla `Cat.hiss`.
                    cat.make_sound(Sound::EntityCatHiss);
                }
            }
            self.is_scared_of_cat = cats_nearby;
        }
        !self.is_scared_of_cat
    }

    fn start(&mut self, _mob: &dyn Mob) {}

    fn stop(&mut self, mob: &dyn Mob) {
        mob.set_mob_target(None, TargetReason::ForgotTarget);
        if let Some(phantom) = self.phantom.upgrade() {
            phantom.swooping.store(false, Ordering::Relaxed);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(phantom) = self.phantom.upgrade() else {
            return;
        };
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        phantom.move_target_point.store(Vector3::new(
            target_pos.x,
            target_pos.y + f64::from(target_entity.height()) * 0.5,
            target_pos.z,
        ));
        let entity = phantom.entity();
        if entity
            .bounding_box
            .load()
            .expand_all(f64::from(0.2f32))
            .intersects(&target_entity.bounding_box.load())
        {
            mob.do_hurt_target(target.as_ref());
            phantom.swooping.store(false, Ordering::Relaxed);
            if !entity.is_silent() {
                entity.world.load().sync_world_event(
                    WorldEvent::SoundPhantomBite,
                    entity.block_pos.load(),
                    0,
                );
            }
        } else if entity.horizontal_collision.load(Ordering::SeqCst)
            || phantom
                .mob_entity
                .living_entity
                .hurt_cooldown
                .load(Ordering::Relaxed)
                > 10
        {
            // `wasHurtRecently`: the 10 tick hurt animation, the first half of the cooldown.
            phantom.swooping.store(false, Ordering::Relaxed);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `PhantomCircleAroundAnchorGoal`.
struct PhantomCircleAroundAnchorGoal {
    phantom: Weak<PhantomEntity>,
    angle: f32,
    distance: f32,
    height: f32,
    clockwise: f32,
}

impl PhantomCircleAroundAnchorGoal {
    fn select_next(&mut self, phantom: &PhantomEntity) {
        let anchor = phantom.anchor_point.load().unwrap_or_else(|| {
            let anchor = phantom.entity().block_pos.load();
            phantom.anchor_point.store(Some(anchor));
            anchor
        });
        self.angle += self.clockwise * 15.0 * DEG_TO_RAD;
        phantom.move_target_point.store(Vector3::new(
            f64::from(anchor.0.x) + f64::from(self.distance * cos(self.angle)),
            f64::from(anchor.0.y) + f64::from(-4.0 + self.height),
            f64::from(anchor.0.z) + f64::from(self.distance * sin(self.angle)),
        ));
    }
}

impl Goal for PhantomCircleAroundAnchorGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.phantom.upgrade().is_some_and(|phantom| {
            mob.get_mob_entity().get_target().is_none() || !phantom.swooping.load(Ordering::Relaxed)
        })
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(phantom) = self.phantom.upgrade() else {
            return;
        };
        let random = phantom.get_random();
        self.distance = 5.0 + random.random::<f32>() * 10.0;
        self.height = -4.0 + random.random::<f32>() * 9.0;
        self.clockwise = if random.random::<bool>() { 1.0 } else { -1.0 };
        self.select_next(&phantom);
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(phantom) = self.phantom.upgrade() else {
            return;
        };
        let random = phantom.get_random();
        if random.random_range(0..self.get_tick_count(350)) == 0 {
            self.height = -4.0 + random.random::<f32>() * 9.0;
        }
        if random.random_range(0..self.get_tick_count(250)) == 0 {
            self.distance += 1.0;
            if self.distance > 15.0 {
                self.distance = 5.0;
                self.clockwise = -self.clockwise;
            }
        }
        if random.random_range(0..self.get_tick_count(450)) == 0 {
            self.angle = random.random::<f32>() * 2.0 * PI;
            self.select_next(&phantom);
        }
        if phantom.touching_target() {
            self.select_next(&phantom);
        }

        let entity = phantom.entity();
        let world = entity.world.load();
        let block_pos = entity.block_pos.load();
        let target_y = phantom.move_target_point.load().y;
        let y = entity.pos.load().y;
        if target_y < y && !world.get_block_state(&block_pos.down()).is_air() {
            self.height = self.height.max(1.0);
            self.select_next(&phantom);
        }
        if target_y > y && !world.get_block_state(&block_pos.up()).is_air() {
            self.height = self.height.min(-1.0);
            self.select_next(&phantom);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

impl Mob for PhantomEntity {
    fn mob_is_flapping(&self) -> bool {
        let entity = self.get_entity();
        // Vanilla `getUniqueFlapTickOffset` spreads the flaps by entity id.
        entity
            .entity_id
            .wrapping_mul(3)
            .wrapping_add(entity.tick_count.load(Ordering::Relaxed))
            % TICKS_PER_FLAP
            == 0
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    // Vanilla `Phantom.travel` flies at 0.2 in the air.
    fn travel(&self, caller: &dyn EntityBase) -> bool {
        self.mob_entity
            .living_entity
            .travel_flying_at(caller, f64::from(0.2f32));
        true
    }

    // Vanilla `PhantomBodyRotationControl` turns the head to the old body rotation and the
    // body to the yaw the move control picked.
    fn post_tick(&self) {
        let entity = self.entity();
        entity.head_yaw.store(entity.body_yaw.load());
        entity.body_yaw.store(entity.yaw.load());
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.anchor_point
            .store(Some(self.entity().block_pos.load().up_height(5)));
        self.set_phantom_size(0);
        self.finalize_spawn_super(group_data)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPhantomAmbient)
    }

    fn mob_init_data_tracker(&self) {
        self.entity()
            .set_synced_data(tracked_data::phantom::ID_SIZE, self.get_phantom_size());
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(anchor) = self.anchor_point.load() {
            nbt.put("anchor_pos", block_pos_to_tag(anchor));
        }
        nbt.put_int("size", self.get_phantom_size());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.anchor_point
            .store(nbt.get("anchor_pos").and_then(block_pos_from_tag));
        self.set_phantom_size(nbt.get_int("size").unwrap_or(0));
    }
}

impl CustomSound for PhantomEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPhantomDeath)
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPhantomHurt)
    }

    fn sound_category(&self) -> SoundCategory {
        SoundCategory::Hostile
    }
}
