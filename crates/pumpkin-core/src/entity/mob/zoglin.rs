use pumpkin_util::random::gameplay::game_random;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick,
    activity::ActivityData,
    behavior::{
        DoNothing, LookAtTargetSink, MoveToTargetSink, Timed, melee_attack, random_stroll, run_one,
        set_entity_look_target_sometimes,
        set_walk_target_from_attack_target_if_target_out_of_reach,
        set_walk_target_from_look_target, start_attacking, stop_attacking_if_target_invalid, utils,
    },
    memory::{MemoryStatus, PackedMemories, types},
    pending::PendingBrainUpdates,
    sensing::{SensorType, is_entity_attackable},
};
use crate::entity::ai::goal::GoalReach;
use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity, MobMetadata,
        hoglin::{hurt_and_throw_target, throw_target},
    },
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

const ATTACK_INTERVAL: i32 = 40;
const BABY_ATTACK_INTERVAL: i32 = 15;
const ATTACK_DURATION: i64 = 200;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.4;
/// `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(1.0)` of the FIGHT activity.
const SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 1.0;
/// `RandomStroll.stroll`'s horizontal range, and the node next to it.
const STROLL_BLOCK_READS: f64 = 11.0;
/// Vanilla `Zoglin.hurtServer` distance for `isOtherTargetMuchFurtherAwayThanCurrentAttackTarget`.
const MUCH_FURTHER_AWAY: f64 = 4.0;

pub struct ZoglinEntity {
    pub mob_entity: MobEntity,
    pub is_baby: AtomicBool,
    pending_brain_updates: PendingBrainUpdates<Self>,
}

impl ZoglinEntity {
    pub const XP_REWARD: u32 = 5;
    const BABY_ATTACK_DAMAGE: f64 = 0.5;

    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Zoglin.defineSynchedData`.
        entity
            .synched_data
            .define(tracked_data::zoglin::DATA_BABY_ID, false);
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = 40.0;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = 0.3;
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(knockback_res) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                knockback_res.base_value = 0.6;
                knockback_res.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(attack_kb) = attributes.get_mut(&Attributes::ATTACK_KNOCKBACK.id) {
                attack_kb.base_value = 1.0;
                attack_kb.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = 6.0;
                damage.dirty.store(true, Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(40.0);

        Arc::new(Self {
            mob_entity,
            is_baby: AtomicBool::new(false),
            pending_brain_updates: PendingBrainUpdates::default(),
        })
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// Vanilla `Zoglin.setBaby`; growing up keeps the lowered attack damage, as in vanilla.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity
            .set_baby_flag(&self.is_baby, tracked_data::zoglin::DATA_BABY_ID, baby);
        if baby {
            self.mob_entity
                .living_entity
                .set_attribute_base(&Attributes::ATTACK_DAMAGE, Self::BABY_ATTACK_DAMAGE);
        }
    }

    /// Vanilla `Zoglin.updateActivity`; returns whether the zoglin just started fighting.
    fn update_activity(&self, brain: &mut BrainState) -> bool {
        let old_activity = brain.get_active_non_core_activity();
        brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
        let new_activity = brain.get_active_non_core_activity();
        self.mob_entity
            .set_attacking(brain.has_memory_value(types::ATTACK_TARGET.id()));
        new_activity == Some(Activity::Fight) && old_activity != Some(Activity::Fight)
    }
}

fn is_adult(mob: &dyn Mob) -> bool {
    !mob.get_mob_entity().living_entity.is_baby()
}

fn is_baby(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().living_entity.is_baby()
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), fight_activity()]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(MoveToTargetSink::default())),
        ],
    )
}

fn idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(start_attacking::create(
                |_| true,
                find_nearest_valid_attack_target,
            )),
            Box::new(set_entity_look_target_sometimes::create(8.0, 30, 60)),
            Box::new(run_one(vec![
                (
                    Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                    2,
                ),
                (
                    Box::new(set_walk_target_from_look_target::create(
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        3,
                    )),
                    2,
                ),
                (Box::new(DoNothing::new(30, 60)), 1),
            ])),
        ],
    )
}

fn fight_activity() -> ActivityData {
    ActivityData::with_priority_start_and_conditions(
        Activity::Fight,
        10,
        vec![
            Box::new(
                set_walk_target_from_attack_target_if_target_out_of_reach::create(
                    SPEED_MULTIPLIER_WHEN_FIGHTING,
                ),
            ),
            Box::new(melee_attack::create_when(is_adult, ATTACK_INTERVAL)),
            Box::new(melee_attack::create_when(is_baby, BABY_ATTACK_INTERVAL)),
            Box::new(stop_attacking_if_target_invalid::create(
                |_, _| false,
                |_, _| {},
                true,
            )),
        ],
        vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
    )
}

/// Vanilla `Zoglin.findNearestValidAttackTarget`: the closest visible living entity that is
/// neither a zoglin nor a creeper.
fn find_nearest_valid_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let ctx = tick.visibility();
    tick.state
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?
        .find_closest(&ctx, |target| {
            let entity_type = target.get_entity().entity_type;
            entity_type != &EntityType::ZOGLIN
                && entity_type != &EntityType::CREEPER
                && is_entity_attackable(&ctx, target.as_ref())
        })
}

/// Vanilla `Zoglin.setAttackTarget`.
fn set_attack_target(brain: &mut BrainState, target: Arc<dyn EntityBase>) {
    brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    brain.set_with_expiry(types::ATTACK_TARGET, target, ATTACK_DURATION);
}

impl Mob for ZoglinEntity {
    fn has_audited_tick(&self) -> bool {
        // An enchanted or special weapon hits more than the attack target.
        self.mob_entity.hits_only_its_target()
    }

    /// The brain's sensors, behaviours and hurt targeting.
    fn audited_species_reach(&self, _speed: f64) -> GoalReach {
        let mob_entity = &self.mob_entity;
        let follow_range = mob_entity
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);
        let melee = mob_entity.melee_attack_max_reach();
        let mut reach = GoalReach {
            // The living entity and player sensors scan the follow range and check the sight to
            // what they find; `MeleeAttack` hits and throws the target within the attack box.
            entities: follow_range.max(melee),
            players: follow_range.max(melee),
            speed: f64::from(SPEED_MULTIPLIER_WHEN_FIGHTING),
            block_reads: follow_range.max(STROLL_BLOCK_READS),
            knockback: melee,
            // `hurtServer` targets an attacker that reaches the zoglin earlier in the tick.
            spans_contacts: true,
            ..GoalReach::default()
        };
        reach.sight.extend(
            mob_entity
                .living_entity
                .get_last_hurt_by_mob_id()
                .filter(|id| *id != 0),
        );
        // Memories keep the last scan and the targets wherever they are now; the behaviours read
        // them and check the sight to them.
        mob_entity.read_brain(|brain| {
            let Some(brain) = brain else {
                return;
            };
            let id = |entity: &Arc<dyn EntityBase>| entity.get_entity().entity_id;
            reach.links.extend(
                brain
                    .get(types::NEAREST_LIVING_ENTITIES)
                    .into_iter()
                    .flatten()
                    .map(id),
            );
            reach.links.extend(
                brain
                    .get(types::NEAREST_PLAYERS)
                    .into_iter()
                    .flatten()
                    .map(|player| player.get_entity().entity_id),
            );
            reach.links.extend(brain.get(types::ATTACK_TARGET).map(id));
            reach.links.extend(
                brain
                    .get(types::LOOK_TARGET)
                    .and_then(|target| target.tracked_entity())
                    .map(id),
            );
            reach.links.extend(
                brain
                    .get(types::WALK_TARGET)
                    .and_then(|target| target.target().tracked_entity())
                    .map(id),
            );
        });
        reach.sight.extend(reach.links.iter().copied());
        reach
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(
            if self
                .mob_entity
                .brain_has_memory_value(types::ATTACK_TARGET.id())
            {
                Sound::EntityZoglinAngry
            } else {
                Sound::EntityZoglinAmbient
            },
        )
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityZoglinStep, 0.15, 1.0);
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        self.is_baby().then(|| MobMetadata::new().baby(true))
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &[],
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
            ],
            activities,
        }
        .make_brain(self, packed)
    }

    /// Vanilla `Zoglin.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let world = self.get_entity().world.load_full();
        let time = world.get_world_age();
        let (started_fighting, target) = self.mob_entity.with_brain(self, |brain| {
            brain.tick(&world, self, time);
            self.pending_brain_updates.run(self, brain);
            (
                self.update_activity(brain),
                brain.get(types::ATTACK_TARGET).cloned(),
            )
        });
        if started_fighting {
            self.make_sound(Sound::EntityZoglinAngry);
        }
        // `getTarget` reads the brain's attack target.
        let current = self.mob_entity.get_target();
        if current.as_ref().map(|t| t.get_entity().entity_id)
            != target.as_ref().map(|t| t.get_entity().entity_id)
        {
            let reason = if target.is_some() {
                TargetReason::Angry
            } else {
                TargetReason::ForgotTarget
            };
            if !self.set_mob_target(target.clone(), reason) && target.is_some() {
                self.mob_entity.set_target(None);
            }
        }
    }

    /// Vanilla `Zoglin.hurtServer`: targets the causing living entity unless the current
    /// target is much closer.
    fn on_damage_with_context(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(attacker) = cause.or(source)
            && attacker.get_living_entity().is_some()
            && let Some(attacker) = self
                .get_entity()
                .world
                .load()
                .get_entity_by_id(attacker.get_entity().entity_id)
        {
            self.pending_brain_updates
                .update(self, move |zoglin, brain| {
                    if zoglin.can_attack(attacker.as_ref())
                        && !utils::is_other_target_much_further_away_than_current_attack_target(
                            brain,
                            zoglin,
                            attacker.as_ref(),
                            MUCH_FURTHER_AWAY,
                        )
                    {
                        set_attack_target(brain, attacker);
                    }
                });
        }
        true
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        if game_random().random::<f32>() < 0.2 {
            self.set_baby(true);
        }
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsBaby", self.is_baby());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
    }

    /// Vanilla `Zoglin.doHurtTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        if target.get_living_entity().is_none() {
            return false;
        }
        let entity = self.get_entity();
        entity.world.load().send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::None,
        );
        self.make_sound(Sound::EntityZoglinAttack);
        hurt_and_throw_target(self, target)
    }

    fn blocked_by_item(&self, defender: &dyn EntityBase, _fully_blocked: bool) {
        if !self.is_baby() {
            throw_target(&self.mob_entity.living_entity, defender);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use crate::entity::passive::pig::PigEntity;
    use pumpkin_util::math::vector3::Vector3;

    #[tokio::test]
    async fn audited_reach_links_a_remembered_target_out_of_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let zoglin = ZoglinEntity::new(Entity::new(
            world.clone(),
            Vector3::new(0.0, 64.0, 0.0),
            &EntityType::ZOGLIN,
        ));
        let pig: Arc<dyn EntityBase> = PigEntity::new(Entity::new(
            world.clone(),
            Vector3::new(100.0, 64.0, 0.0),
            &EntityType::PIG,
        ));
        let pig_id = pig.get_entity().entity_id;
        zoglin
            .mob_entity
            .with_brain(zoglin.as_ref(), |brain| set_attack_target(brain, pig));
        let reach = zoglin
            .mob_entity
            .audited_reach(zoglin.as_ref())
            .ok_or("missing zoglin reach")?;
        assert!(reach.links.contains(&pig_id));
        assert!(reach.sight.contains(&pig_id));
        world.level.shutdown().await;
        Ok(())
    }
}
