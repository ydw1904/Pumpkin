use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::move_control::{MoveControl, Operation};
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::track_target::TrackTargetGoal;
use crate::entity::ai::goal::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::raider;
use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData};
use crate::entity::projectile::owner::EntityReference;
use pumpkin_data::data_component_impl::EquipmentSlot;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{look_at_entity::LookAtEntityGoal, swim::SwimGoal},
    mob::{Mob, MobEntity},
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

const VEX_HUNTS: &[Hunt] = &[Hunt::new(3, TargetMatch::Type(&EntityType::PLAYER), true)];

/// Vanilla `Vex.TICKS_PER_FLAP`.
const TICKS_PER_FLAP: i32 = 4;
const FLAG_IS_CHARGING: u8 = 1;
/// Vanilla `reducedTickDelay(7)`, the chance both vex movement goals start with.
const MOVE_GOAL_CHANCE: i32 = to_goal_ticks(7);

/// The state vanilla `Vex` keeps beside the shared mob fields, shared with its goals.
#[derive(Default)]
pub struct VexState {
    flags: AtomicU8,
    owner: EntityReference,
    bound_origin: AtomicCell<Option<BlockPos>>,
    has_limited_life: AtomicBool,
    limited_life_ticks: AtomicI32,
}

impl VexState {
    fn is_charging(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & FLAG_IS_CHARGING != 0
    }

    fn set_is_charging(&self, entity: &Entity, value: bool) {
        let flags = self.flags.load(Ordering::Relaxed);
        let flags = if value {
            flags | FLAG_IS_CHARGING
        } else {
            flags & !FLAG_IS_CHARGING
        };
        self.flags.store(flags, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::vex::DATA_FLAGS_ID, flags as i8);
    }

    fn owner(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        self.owner
            .get(world)
            .filter(|owner| owner.get_living_entity().is_some())
    }
}

pub struct VexEntity {
    pub mob_entity: MobEntity,
    state: Arc<VexState>,
}

impl VexEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let state = Arc::new(VexState::default());
        // Vanilla `Vex.defineSynchedData`, so the default isn't sent.
        entity
            .synched_data
            .define(tracked_data::vex::DATA_FLAGS_ID, 0i8);
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(VexMoveControl::default());
        let vex = Self { mob_entity, state };
        let mob_arc = Arc::new(vex);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                4,
                Box::new(VexChargeAttackGoal {
                    state: mob_arc.state.clone(),
                }),
            );
            goal_selector.add_goal(
                8,
                Box::new(VexRandomMoveGoal {
                    state: mob_arc.state.clone(),
                }),
            );
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak,
                    &EntityType::PLAYER,
                    3.0,
                    1.0,
                    false,
                )),
            );
            goal_selector.add_goal(10, LookAtEntityGoal::mobs(8.0));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(1, raider::revenge_goal());
            target_selector.add_goal(
                2,
                Box::new(VexCopyOwnerTargetGoal::new(mob_arc.state.clone())),
            );
            hunting::apply(target_selector, &mob_arc.mob_entity, VEX_HUNTS);
        };

        mob_arc
    }

    /// Vanilla `Vex.setOwner`.
    pub fn set_owner(&self, owner: &Entity) {
        self.state.owner.set(Some(owner));
    }

    /// Vanilla `Vex.setBoundOrigin`.
    pub fn set_bound_origin(&self, bound_origin: Option<BlockPos>) {
        self.state.bound_origin.store(bound_origin);
    }

    /// Vanilla `Vex.setLimitedLife`.
    pub fn set_limited_life(&self, life_ticks: i32) {
        self.state.has_limited_life.store(true, Ordering::Relaxed);
        self.state
            .limited_life_ticks
            .store(life_ticks, Ordering::Relaxed);
    }
}

impl Mob for VexEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_is_flapping(&self) -> bool {
        self.get_entity().tick_count.load(Ordering::Relaxed) % TICKS_PER_FLAP == 0
    }

    /// Vanilla `Vex.tick`, the part before `super.tick()`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.get_entity().no_physics.store(true, Ordering::Relaxed);
    }

    /// Vanilla `Vex.tick`, the part after `super.tick()`.
    fn post_tick(&self) {
        let entity = self.get_entity();
        entity.no_physics.store(false, Ordering::Relaxed);
        entity.set_has_no_gravity(true);
        if self.state.has_limited_life.load(Ordering::Relaxed)
            && self
                .state
                .limited_life_ticks
                .fetch_sub(1, Ordering::Relaxed)
                - 1
                <= 0
        {
            self.state.limited_life_ticks.store(20, Ordering::Relaxed);
            self.damage(self, 1.0, DamageType::STARVE);
        }
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let difficulty = RegionalDifficulty::at(world, self.get_entity().pos.load());
        self.populate_default_equipment_slots(world, &difficulty);
        self.populate_default_equipment_enchantments(&difficulty);
        self.finalize_spawn_super(group_data)
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        self.mob_entity.set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::IRON_SWORD),
        );
        self.mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(EquipmentSlot::MAIN_HAND, 0.0);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(pos) = self.state.bound_origin.load() {
            nbt.put(
                "bound_pos",
                pumpkin_nbt::tag::NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
            );
        }
        if self.state.has_limited_life.load(Ordering::Relaxed) {
            nbt.put_int(
                "life_ticks",
                self.state.limited_life_ticks.load(Ordering::Relaxed),
            );
        }
        self.state.owner.write_nbt(nbt, "owner");
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let bound_origin = nbt
            .get_int_array("bound_pos")
            .filter(|pos| pos.len() == 3)
            .map(|pos| BlockPos::new(pos[0], pos[1], pos[2]));
        self.state.bound_origin.store(bound_origin);
        if let Some(life_ticks) = nbt.get_int("life_ticks") {
            self.set_limited_life(life_ticks);
        } else {
            self.state.has_limited_life.store(false, Ordering::Relaxed);
        }
        self.state.owner.read_nbt(nbt, "owner");
    }
}

/// Vanilla `Vex.VexChargeAttackGoal`.
struct VexChargeAttackGoal {
    state: Arc<VexState>,
}

impl VexChargeAttackGoal {
    fn charge_at(mob: &dyn Mob, target: &dyn EntityBase) {
        let eye = target.get_entity().get_eye_pos();
        mob.get_mob_entity()
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_wanted_position(eye.x, eye.y, eye.z, 1.0);
    }

    fn has_wanted(mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_wanted()
    }
}

impl Goal for VexChargeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        if !target.get_entity().is_alive()
            || Self::has_wanted(mob)
            || mob.get_random().random_range(0..MOVE_GOAL_CHANCE) != 0
        {
            return false;
        }
        mob.get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load())
            > 4.0
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        Self::has_wanted(mob)
            && self.state.is_charging()
            && mob
                .get_mob_entity()
                .get_target()
                .is_some_and(|target| target.get_entity().is_alive())
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = mob.get_mob_entity().get_target() {
            Self::charge_at(mob, target.as_ref());
        }
        self.state.set_is_charging(mob.get_entity(), true);
        mob.play_entity_sound(Sound::EntityVexCharge, 1.0, 1.0);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.state.set_is_charging(mob.get_entity(), false);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let entity = mob.get_entity();
        let target_entity = target.get_entity();
        if entity
            .bounding_box
            .load()
            .intersects(&target_entity.bounding_box.load())
        {
            mob.do_hurt_target(target.as_ref());
            self.state.set_is_charging(entity, false);
        } else if entity
            .pos
            .load()
            .squared_distance_to_vec(&target_entity.pos.load())
            < 9.0
        {
            Self::charge_at(mob, target.as_ref());
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Vex.VexRandomMoveGoal`.
struct VexRandomMoveGoal {
    state: Arc<VexState>,
}

impl Goal for VexRandomMoveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !VexChargeAttackGoal::has_wanted(mob)
            && mob.get_random().random_range(0..MOVE_GOAL_CHANCE) == 0
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let bound_origin = self
            .state
            .bound_origin
            .load()
            .unwrap_or_else(|| entity.block_pos.load());
        let world = entity.world.load();
        let rng = mob.get_random();
        for _ in 0..3 {
            let x = rng.random_range(0..15) - 7;
            let y = rng.random_range(0..11) - 5;
            let z = rng.random_range(0..15) - 7;
            let test_pos = bound_origin.offset(Vector3::new(x, y, z));
            if world.get_block_state(&test_pos).is_air() {
                let wanted = Vector3::new(
                    f64::from(test_pos.0.x) + 0.5,
                    f64::from(test_pos.0.y) + 0.5,
                    f64::from(test_pos.0.z) + 0.5,
                );
                let mob_entity = mob.get_mob_entity();
                mob_entity
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_wanted_position(wanted.x, wanted.y, wanted.z, 0.25);
                if mob_entity.get_target().is_none() {
                    mob_entity
                        .look_control
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .look_at_with_range(wanted.x, wanted.y, wanted.z, 180.0, 20.0);
                }
                break;
            }
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Vex.VexCopyOwnerTargetGoal`.
struct VexCopyOwnerTargetGoal {
    track_target_goal: TrackTargetGoal,
    state: Arc<VexState>,
    copy_owner_targeting: TargetPredicate,
}

impl VexCopyOwnerTargetGoal {
    fn new(state: Arc<VexState>) -> Self {
        Self {
            track_target_goal: TrackTargetGoal::with_default(false),
            state,
            copy_owner_targeting: TargetPredicate::create_non_attackable().ignore_visibility(),
        }
    }

    fn owner_target(&self, mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let world = mob.get_entity().world.load();
        self.state
            .owner(&world)?
            .get_mob()?
            .get_mob_entity()
            .get_target()
    }
}

impl Goal for VexCopyOwnerTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let target = self.owner_target(mob);
        self.track_target_goal
            .can_track(mob, target.as_deref(), &self.copy_owner_targeting)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.track_target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        let target = self.owner_target(mob);
        self.track_target_goal
            .start_targeting(mob, target, TargetReason::OwnerAttackedTarget);
        self.track_target_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.track_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.track_target_goal.controls()
    }
}

/// Vanilla `Vex.VexMoveControl`.
#[derive(Default)]
struct VexMoveControl {
    inner: MoveControl,
}

impl Control for VexMoveControl {}

impl MoveControlTrait for VexMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if self.inner.operation != Operation::MoveTo {
            return;
        }
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let delta = Vector3::new(
            self.inner.wanted_x - pos.x,
            self.inner.wanted_y - pos.y,
            self.inner.wanted_z - pos.z,
        );
        let delta_length = delta.length();
        if delta_length < entity.bounding_box.load().get_average_side_length() {
            self.inner.operation = Operation::Wait;
            entity.velocity.store(entity.velocity.load() * 0.5);
            return;
        }
        let velocity =
            entity.velocity.load() + delta * (self.inner.speed_modifier * 0.05 / delta_length);
        entity.velocity.store(velocity);
        let (dx, dz) =
            mob.get_mob_entity()
                .get_target()
                .map_or((velocity.x, velocity.z), |target| {
                    let target_pos = target.get_entity().pos.load();
                    (target_pos.x - pos.x, target_pos.z - pos.z)
                });
        let yaw = -(dx.atan2(dz) as f32) * (180.0 / std::f32::consts::PI);
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.inner.strafe(forwards, right);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }
}
