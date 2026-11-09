use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::behavior::neutral::apply_targets,
    ai::goal::{
        Controls, Goal, GoalReach, active_target::TargetCondition, escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        polar_bear_attack_players::PolarBearAttackPlayersGoal, revenge::RevengeGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    passive::animal::Animal,
    player::Player,
};

fn is_baby(mob: &dyn Mob) -> bool {
    mob.as_ageable().is_some_and(AgeableMob::is_baby)
}

const POLAR_BEAR_HUNTS: &[Hunt] =
    &[Hunt::new(4, TargetMatch::Type(&EntityType::FOX), true).when(TargetCondition::Adult)];

pub struct PolarBearEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub neutral_data: NeutralData,
    pub standing: AtomicBool,
    warning_sound_ticks: AtomicI32,
}

/// Vanilla `PolarBear.playWarningSound` cooldown.
const WARNING_SOUND_COOLDOWN: i32 = 40;

impl PolarBearEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `PolarBear.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            false,
        );
        let mob_entity = MobEntity::new(entity);
        let polar_bear = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            neutral_data: NeutralData::default(),
            standing: AtomicBool::new(false),
            warning_sound_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(polar_bear);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(PolarBearMeleeAttackGoal::new()));
            goal_selector.add_goal(
                1,
                EscapeDangerGoal::environmental(2.0)
                    .babies_panicking_at(&tag::DamageType::MINECRAFT_PANIC_CAUSES),
            );
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .raising_alarm_when(is_baby)
                        .alerting_only(|other| !is_baby(other)),
                ),
            );
            target_selector.add_goal(2, PolarBearAttackPlayersGoal::new(&mob_arc.mob_entity));
            apply_targets(target_selector, &mob_arc.mob_entity, 3, 5, false);
            hunting::apply(target_selector, &mob_arc.mob_entity, POLAR_BEAR_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn is_standing(&self) -> bool {
        self.standing.load(Ordering::Relaxed)
    }

    pub fn set_standing(&self, standing: bool) {
        self.standing.store(standing, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            standing,
        );
    }

    fn play_warning_sound(&self) {
        if self.warning_sound_ticks.load(Ordering::Relaxed) <= 0 {
            self.make_sound(Sound::EntityPolarBearWarning);
            self.warning_sound_ticks
                .store(WARNING_SOUND_COOLDOWN, Ordering::Relaxed);
        }
    }
}

fn as_polar_bear(mob: &dyn Mob) -> Option<&PolarBearEntity> {
    mob.cast_any().downcast_ref::<PolarBearEntity>()
}

/// Vanilla `PolarBear.PolarBearMeleeAttackGoal`: the bear rears up and growls while the target is
/// close but the attack is still cooling down.
struct PolarBearMeleeAttackGoal {
    melee: MeleeAttackGoal,
}

impl PolarBearMeleeAttackGoal {
    fn new() -> Self {
        Self {
            melee: MeleeAttackGoal::new(1.25, true),
        }
    }

    fn check_and_perform_attack(&mut self, mob: &dyn Mob, target: &dyn EntityBase) {
        let Some(bear) = as_polar_bear(mob) else {
            return;
        };
        if self.melee.can_perform_attack(mob, target) {
            self.melee.reset_attack_cooldown();
            mob.do_hurt_target(target);
            bear.set_standing(false);
            return;
        }
        let warn_distance = f64::from(target.get_entity().width()) + 3.0;
        let distance_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        if distance_sq < warn_distance * warn_distance {
            if self.melee.is_time_to_attack() {
                bear.set_standing(false);
                self.melee.reset_attack_cooldown();
            }
            if self.melee.cooldown <= 10 {
                bear.set_standing(true);
                bear.play_warning_sound();
            }
        } else {
            self.melee.reset_attack_cooldown();
            bear.set_standing(false);
        }
    }
}

impl Goal for PolarBearMeleeAttackGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        // Standing up and the warning sound only touch the bear itself.
        self.melee.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.melee.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.melee.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.melee.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(bear) = as_polar_bear(mob) {
            bear.set_standing(false);
        }
        self.melee.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.melee.tick_chase(mob) {
            self.check_and_perform_attack(mob, target.as_ref());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        self.melee.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.melee.controls()
    }
}

impl AgeableMob for PolarBearEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::POLAR_BEAR_BABY_SPAWN_CHANCE
    }
}

impl Animal for PolarBearEntity {
    fn is_food(&self, _item_stack: &ItemStack) -> bool {
        false
    }
}

impl NeutralMob for PolarBearEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }
}

impl Mob for PolarBearEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.mob_entity.living_entity.is_baby() {
            Sound::EntityPolarBearAmbientBaby
        } else {
            Sound::EntityPolarBearAmbient
        })
    }

    fn post_tick(&self) {
        let ticks = self.warning_sound_ticks.load(Ordering::Relaxed);
        if ticks > 0 {
            self.warning_sound_ticks.store(ticks - 1, Ordering::Relaxed);
        }
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPolarBearStep, 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::polar_bear::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            self.is_standing(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}
