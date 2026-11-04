use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::goal::reset_universal_anger::ResetUniversalAngerGoal;
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::util::goal_utils;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::{
    Entity, EntityBase,
    ai::behavior::hunting::{self, Hunt},
    ai::goal::{
        active_target::TargetCondition, revenge::RevengeGoal, spear_use::SpearUseGoal,
        wander_around::WanderAroundGoal, zombie_attack::ZombieAttackGoal,
    },
    ai::target_match::TargetMatch,
    mob::{
        Mob, MobEntity, MobMetadata,
        equipment::RegionalDifficulty,
        neutral::{NeutralData, NeutralMob},
        spawn::{EntitySpawnReason, SpawnGroupData},
        zombie::ZombieEntityBase,
    },
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::plugin::api::events::entity::pig_zombie_anger::PigZombieAngerEvent;
use crate::world::World;
use crate::world::entity_islands::HurtReach;

/// Vanilla `ALERT_INTERVAL`: 4 to 6 seconds.
const ALERT_INTERVAL: std::ops::RangeInclusive<i32> = 80..=120;
/// Vanilla `FIRST_ANGER_SOUND_DELAY`: 0 to 1 second.
const FIRST_ANGER_SOUND_DELAY: std::ops::RangeInclusive<i32> = 0..=20;
const SPEED_MODIFIER_ATTACKING_ID: &str = "minecraft:attacking";
const SPEED_MODIFIER_ATTACKING_AMOUNT: f64 = 0.05;

/// Vanilla `ZombifiedPiglin`, a `Zombie` that is also a `NeutralMob`.
pub struct ZombifiedPiglinEntity {
    entity: Arc<ZombieEntityBase>,
    neutral_data: NeutralData,
    ticks_until_next_alert: AtomicI32,
    play_first_anger_sound_in: AtomicI32,
}

impl ZombifiedPiglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::without_behaviour_goals(entity, false);
        entity
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_pathfinding_malus(PathType::Lava, 8.0);
        let piglin = Arc::new(Self {
            entity,
            neutral_data: NeutralData::default(),
            ticks_until_next_alert: AtomicI32::new(0),
            play_first_anger_sound_in: AtomicI32::new(0),
        });
        piglin.add_behaviour_goals();
        piglin
    }

    /// Vanilla `ZombifiedPiglin.addBehaviourGoals`.
    fn add_behaviour_goals(&self) {
        let mob_entity = &self.entity.mob_entity;
        let goal_selector = &mob_entity.goals_selector;
        goal_selector.add_goal(1, SpearUseGoal::new(1.0, 1.0, 10.0, 2.0));
        goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
        goal_selector.add_goal(7, Box::new(WanderAroundGoal::water_avoiding(1.0)));

        let target_selector = &mob_entity.target_selector;
        target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alerting_others()));
        hunting::apply(
            target_selector,
            mob_entity,
            &[Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true)
                .when(TargetCondition::AngryAt)],
        );
        target_selector.add_goal(3, ResetUniversalAngerGoal::new(true));
    }

    fn speed_modifier_attacking() -> Modifier {
        Modifier {
            id: SPEED_MODIFIER_ATTACKING_ID.to_string(),
            amount: SPEED_MODIFIER_ATTACKING_AMOUNT,
            operation: ModifierOperation::Add,
        }
    }

    /// Vanilla `maybePlayFirstAngerSound`.
    fn maybe_play_first_anger_sound(&self) {
        let remaining = self.play_first_anger_sound_in.load(Ordering::Relaxed);
        if remaining > 0 {
            self.play_first_anger_sound_in
                .store(remaining - 1, Ordering::Relaxed);
            if remaining == 1 {
                self.play_anger_sound();
            }
        }
    }

    /// Vanilla `playAngerSound`.
    fn play_anger_sound(&self) {
        let pitch = self.entity.mob_entity.living_entity.get_pitch() * 1.8;
        self.play_entity_sound(Sound::EntityZombifiedPiglinAngry, 2.0, pitch);
    }

    /// Vanilla `maybeAlertOthers`.
    fn maybe_alert_others(&self, target: &Arc<dyn EntityBase>) {
        let remaining = self.ticks_until_next_alert.load(Ordering::Relaxed);
        if remaining > 0 {
            self.ticks_until_next_alert
                .store(remaining - 1, Ordering::Relaxed);
            return;
        }

        if self.has_line_of_sight(target.get_entity()) {
            self.alert_others(target);
        }

        self.ticks_until_next_alert.store(
            game_random().random_range(ALERT_INTERVAL),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `alertOthers`: hands the target to nearby zombified piglins without one.
    fn alert_others(&self, target: &Arc<dyn EntityBase>) {
        for other in goal_utils::nearby_same_class(self) {
            let Some(other_mob) = other.get_mob() else {
                continue;
            };
            if other_mob.get_mob_entity().get_target().is_some()
                || other.is_allied_to(target.as_ref())
            {
                continue;
            }
            other_mob.set_mob_target(Some(target.clone()), TargetReason::PigZombieTarget);
        }
    }
}

impl NeutralMob for ZombifiedPiglinEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }

    fn approve_grudge(&self, target: &dyn EntityBase, ticks: i32) -> Option<i32> {
        let entity = &self.entity.mob_entity.living_entity.entity;
        let Some(server) = entity.world.load().server.upgrade() else {
            return Some(ticks);
        };
        let mut event =
            PigZombieAngerEvent::new(entity.entity_id, Some(target.get_entity().entity_id), ticks);
        server.plugin_manager.fire_blocking(&server, &mut event);
        (!event.cancelled).then_some(event.new_anger)
    }

    fn updates_anger_in_ai_step(&self) -> bool {
        true
    }
}

impl Mob for ZombifiedPiglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn xp_reward(&self) -> u32 {
        self.entity.xp_reward()
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.entity.mob_metadata()
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.entity.finalize_zombie_spawn(self, world, group_data)
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        let weapon = if game_random().random_range(0..20) == 0 {
            &Item::GOLDEN_SPEAR
        } else {
            &Item::GOLDEN_SWORD
        };
        self.entity
            .mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon));
    }

    fn can_hold_item(&self, stack: &ItemStack) -> bool {
        self.entity.can_hold_item(stack)
    }

    // Vanilla `ZombifiedPiglin.wantsToPickUp` skips the zombie's glow ink sac check.
    fn wants_to_pick_up(&self, stack: &ItemStack) -> bool {
        self.can_hold_item(stack)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityZombieStep, 0.15, 1.0);
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_angry() {
            Sound::EntityZombifiedPiglinAngry
        } else {
            Sound::EntityZombifiedPiglinAmbient
        })
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }

    fn killed_entity(&self, victim: &dyn EntityBase) -> bool {
        self.entity.killed_entity(victim)
    }

    fn on_damage_with_context(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.entity.call_reinforcements(cause.or(source));
        true
    }

    fn hurt_reach(&self) -> Option<HurtReach> {
        self.entity.reinforcement_hurt_reach()
    }

    fn additional_tick_movement(&self) -> f64 {
        self.entity.reinforcement_reach()
    }

    /// Vanilla `ZombifiedPiglin.setTarget`: a first target restarts the anger sound and alert
    /// timers.
    fn apply_mob_target(&self, target: Option<Arc<dyn EntityBase>>) {
        let mob_entity = &self.entity.mob_entity;
        if mob_entity.get_target().is_none() && target.is_some() {
            let rng = game_random();
            self.play_first_anger_sound_in
                .store(rng.random_range(FIRST_ANGER_SOUND_DELAY), Ordering::Relaxed);
            self.ticks_until_next_alert
                .store(rng.random_range(ALERT_INTERVAL), Ordering::Relaxed);
        }
        *mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    /// Vanilla `ZombifiedPiglin.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let living = &self.entity.mob_entity.living_entity;
        if self.is_angry() {
            if !self.entity.is_baby()
                && !living.has_attribute_modifier(
                    &Attributes::MOVEMENT_SPEED,
                    SPEED_MODIFIER_ATTACKING_ID,
                )
            {
                living.update_attribute(&Attributes::MOVEMENT_SPEED, |speed| {
                    speed.add_or_replace_modifier(Self::speed_modifier_attacking());
                });
            }
            self.maybe_play_first_anger_sound();
        } else if living
            .has_attribute_modifier(&Attributes::MOVEMENT_SPEED, SPEED_MODIFIER_ATTACKING_ID)
        {
            living.update_attribute(&Attributes::MOVEMENT_SPEED, |speed| {
                speed.remove_modifier(SPEED_MODIFIER_ATTACKING_ID);
            });
        }

        self.update_persistent_anger();
        if let Some(target) = self.entity.mob_entity.get_target() {
            self.maybe_alert_others(&target);
        }
    }
}
