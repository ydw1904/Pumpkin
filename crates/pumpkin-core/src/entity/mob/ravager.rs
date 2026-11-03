use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;

use crate::entity::ageable::AgeableMob;
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider, RaiderCelebrationGoal,
            RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};

const RAVAGER_HUNTS: &[Hunt] = &[
    Hunt::new(3, TargetMatch::Type(&EntityType::PLAYER), true),
    Hunt::new(4, TargetMatch::VILLAGER_LIKE, true).only_mobs(is_adult),
    Hunt::new(4, TargetMatch::Type(&EntityType::IRON_GOLEM), true),
];

const BASE_MOVEMENT_SPEED: f64 = 0.3;
const ATTACK_MOVEMENT_SPEED: f64 = 0.35;
pub const ATTACK_DURATION: i32 = 10;
pub const STUN_DURATION: i32 = 40;

fn is_adult(mob: &dyn Mob) -> bool {
    !mob.as_ageable().is_some_and(AgeableMob::is_baby)
}

pub struct RavagerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    attack_tick: AtomicI32,
    stunned_tick: AtomicI32,
    roar_tick: AtomicI32,
}

impl RavagerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let ravager = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            attack_tick: AtomicI32::new(0),
            stunned_tick: AtomicI32::new(0),
            roar_tick: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(ravager);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Vanilla registration order: `PatrollingMonster`, `Raider`, then `Ravager`.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::water_avoiding(0.4)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(10, LookAtEntityGoal::mobs(8.0));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(2, raider::revenge_goal());
            hunting::apply(target_selector, &mob_arc.mob_entity, RAVAGER_HUNTS);
        };

        mob_arc
    }

    /// Vanilla `Ravager.roar`: hurts and throws back the living entities around it.
    fn roar(&self) {
        let entity = self.get_entity();
        if !entity.is_alive() {
            return;
        }
        let world = entity.world.load();
        let mob_griefing = world.level_info.load().game_rules.mob_griefing;
        let area = entity.bounding_box.load().expand(4.0, 4.0, 4.0);
        for target in world.get_entities_and_players_at_box(&area) {
            let target_entity = target.get_entity();
            if target_entity.entity_type == &EntityType::RAVAGER
                || !target_entity.is_alive()
                || target.get_living_entity().is_none()
                || (!mob_griefing && target_entity.entity_type == &EntityType::ARMOR_STAND)
            {
                continue;
            }
            if !target_entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_ILLAGER)
            {
                target.damage_with_context(
                    target.as_ref(),
                    6.0,
                    DamageType::MOB_ATTACK,
                    None,
                    Some(self),
                    Some(self),
                );
            }
            // Clients knock themselves back on the roar entity event.
            if target.get_player().is_none() {
                self.strong_knockback(target_entity);
            }
        }
        world.emit_game_event(GameEvent::EntityAction.name(), entity.pos.load());
        world.send_entity_status(entity, EntityStatus::RavagerRoared, ActorEventID::None);
    }

    /// Vanilla `Ravager.strongKnockback`.
    fn strong_knockback(&self, target: &Entity) {
        let pos = self.get_entity().pos.load();
        let target_pos = target.pos.load();
        let dx = target_pos.x - pos.x;
        let dz = target_pos.z - pos.z;
        let distance_sq = dx.mul_add(dx, dz * dz).max(0.001);
        target.add_velocity(Vector3::new(
            dx / distance_sq * 4.0,
            0.2,
            dz / distance_sq * 4.0,
        ));
    }
}

impl Mob for RavagerEntity {
    /// Vanilla `Ravager.checkSpawnObstruction`: only liquid gets in the way.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        !level.contains_any_liquid(self.get_entity().bounding_box.load())
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityRavagerStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        nbt.put_int("AttackTick", self.attack_tick.load(Ordering::Relaxed));
        nbt.put_int("StunTick", self.stunned_tick.load(Ordering::Relaxed));
        nbt.put_int("RoarTick", self.roar_tick.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        self.attack_tick
            .store(nbt.get_int("AttackTick").unwrap_or(0), Ordering::Relaxed);
        self.stunned_tick
            .store(nbt.get_int("StunTick").unwrap_or(0), Ordering::Relaxed);
        self.roar_tick
            .store(nbt.get_int("RoarTick").unwrap_or(0), Ordering::Relaxed);
    }

    fn is_mob_immobile(&self) -> bool {
        self.mob_entity.living_entity.is_dead_or_dying()
            || self.attack_tick.load(Ordering::Relaxed) > 0
            || self.stunned_tick.load(Ordering::Relaxed) > 0
            || self.roar_tick.load(Ordering::Relaxed) > 0
    }

    fn has_line_of_sight(&self, target: &Entity) -> bool {
        if self.stunned_tick.load(Ordering::Relaxed) > 0
            || self.roar_tick.load(Ordering::Relaxed) > 0
        {
            return false;
        }
        let mob_entity = &self.mob_entity;
        mob_entity
            .sensing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_line_of_sight(&mob_entity.living_entity, target)
    }

    /// Vanilla `Ravager.aiStep` after `super.aiStep()`.
    fn mob_ai_step(&self) {
        let living = &self.mob_entity.living_entity;
        if !living.entity.is_alive() {
            return;
        }
        let speed = living.get_attribute_base(&Attributes::MOVEMENT_SPEED);
        let new_speed = if self.is_mob_immobile() {
            0.0
        } else {
            let max_speed = if self.mob_entity.get_target().is_some() {
                ATTACK_MOVEMENT_SPEED
            } else {
                BASE_MOVEMENT_SPEED
            };
            0.1f64.mul_add(max_speed - speed, speed)
        };
        if new_speed != speed {
            living.set_attribute_base(&Attributes::MOVEMENT_SPEED, new_speed);
        }

        if self.roar_tick.load(Ordering::Relaxed) > 0
            && self.roar_tick.fetch_sub(1, Ordering::Relaxed) - 1 == 10
        {
            self.roar();
        }
        if self.attack_tick.load(Ordering::Relaxed) > 0 {
            self.attack_tick.fetch_sub(1, Ordering::Relaxed);
        }
        // `stunEffect` only adds particles on the client.
        if self.stunned_tick.load(Ordering::Relaxed) > 0
            && self.stunned_tick.fetch_sub(1, Ordering::Relaxed) - 1 == 0
        {
            self.play_entity_sound(Sound::EntityRavagerRoar, 1.0, 1.0);
            self.roar_tick.store(20, Ordering::Relaxed);
        }
    }

    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        self.attack_tick.store(ATTACK_DURATION, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::StartAttacking,
        );
        self.play_entity_sound(Sound::EntityRavagerAttack, 1.0, 1.0);
        let damaged = self.mob_entity.try_attack(entity, target);
        if damaged {
            self.on_attack(target);
        }
        damaged
    }

    /// Vanilla `Ravager.blockedByItem`: half the time the ravager is stunned, otherwise it
    /// throws the defender back.
    fn blocked_by_item(&self, defender: &dyn EntityBase, _fully_blocked: bool) {
        if self.roar_tick.load(Ordering::Relaxed) != 0 {
            return;
        }
        if game_random().random::<f64>() < 0.5 {
            self.stunned_tick.store(STUN_DURATION, Ordering::Relaxed);
            self.play_entity_sound(Sound::EntityRavagerStunned, 1.0, 1.0);
            let entity = self.get_entity();
            entity.world.load().send_entity_status(
                entity,
                EntityStatus::RavagerStunned,
                ActorEventID::None,
            );
            defender.push(self);
        } else {
            self.strong_knockback(defender.get_entity());
        }
        defender
            .get_entity()
            .velocity_dirty
            .store(true, Ordering::SeqCst);
    }
}

impl PatrollingMonster for RavagerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }

    fn can_be_leader(&self) -> bool {
        false
    }
}

impl Raider for RavagerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityRavagerCelebrate
    }
}
