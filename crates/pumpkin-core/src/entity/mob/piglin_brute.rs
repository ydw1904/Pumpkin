use std::sync::Arc;

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::gameplay::game_random;

use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick,
    activity::ActivityData,
    behavior::{
        DoNothing, LookAtTargetSink, MoveToTargetSink, Timed, door,
        interact::{
            interact_with, set_entity_look_target as set_type_look_target, set_look_and_interact,
        },
        melee_attack,
        poi_walk::{stroll_around_poi, stroll_to_poi},
        random_stroll, run_one, set_entity_look_target,
        set_walk_target_from_attack_target_if_target_out_of_reach, start_attacking,
        stop_attacking_if_target_invalid,
    },
    memory::{GlobalPos, MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::{Sensor, SensorType},
};
use crate::entity::{
    Entity, EntityBase,
    mob::{
        Mob, MobEntity,
        equipment::RegionalDifficulty,
        neutral::find_by_uuid,
        piglin_ai::{
            AbstractPiglin, PiglinAi, PiglinAnger, PiglinZombification, as_abstract_piglin,
        },
    },
};
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;

const MELEE_ATTACK_COOLDOWN: i32 = 20;
const ACTIVITY_SOUND_LIKELIHOOD_PER_TICK: f32 = 0.0125;
const MAX_LOOK_DIST: f32 = 8.0;
const INTERACTION_RANGE: i32 = 8;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.6;
/// `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(1.0)` of the FIGHT activity.
const SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 1.0;
const HOME_CLOSE_ENOUGH_DISTANCE: i32 = 2;
const HOME_TOO_FAR_DISTANCE: i32 = 100;
const HOME_STROLL_AROUND_DISTANCE: i32 = 5;

pub struct PiglinBruteEntity {
    pub mob_entity: MobEntity,
    pub zombification: PiglinZombification,
    pub anger: PiglinAnger,
    /// The active non-core activity, for `PiglinAi.isIdle`.
    activity: AtomicCell<Option<Activity>>,
}

impl PiglinBruteEntity {
    pub const XP_REWARD: u32 = 20;

    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `AbstractPiglin.defineSynchedData`.
        crate::entity::mob::piglin::define_abstract_piglin_synched_data(&entity);
        let mut mob_entity = MobEntity::new(entity);
        crate::entity::mob::piglin::apply_abstract_piglin_navigation(&mut mob_entity);
        Arc::new(Self {
            mob_entity,
            zombification: PiglinZombification::default(),
            anger: PiglinAnger::default(),
            activity: AtomicCell::new(None),
        })
    }

    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.zombification.is_immune()
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.zombification.set_immune(&self.mob_entity, immune);
    }

    #[must_use]
    pub fn check_piglin_brute_spawn_rules(world: &World, pos: &BlockPos) -> bool {
        let below = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        let state = world.get_block_state(&below);
        state.id != Block::NETHER_WART_BLOCK.default_state.id
    }

    /// Vanilla `PiglinBruteAi.initMemories`.
    fn init_memories(&self, world: &World) {
        let Some(dimension) = Dimension::from_name(world.dimension.minecraft_name) else {
            return;
        };
        let home = GlobalPos::new(dimension, self.get_entity().block_pos.load());
        self.mob_entity
            .with_brain(self, |brain| brain.set(types::HOME, home));
    }

    /// Vanilla `PiglinBruteAi.updateActivity`; returns whether the brute just started fighting.
    fn update_activity(&self, brain: &mut BrainState) -> bool {
        let old_activity = brain.get_active_non_core_activity();
        brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
        let new_activity = brain.get_active_non_core_activity();
        self.activity.store(new_activity);
        self.mob_entity
            .set_attacking(brain.has_memory_value(types::ATTACK_TARGET.id()));
        old_activity != new_activity && new_activity == Some(Activity::Fight)
    }

    fn is_fighting(&self) -> bool {
        self.activity.load() == Some(Activity::Fight)
    }
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), fight_activity()]
}

fn core_activity() -> ActivityData {
    // `StopBeingAngryIfTargetDead` runs on `PiglinAnger` before the brain tick.
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(MoveToTargetSink::default())),
            Box::new(door::create()),
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
            Box::new(run_one(vec![
                (
                    Box::new(set_type_look_target::for_type(
                        &EntityType::PLAYER,
                        MAX_LOOK_DIST,
                    )),
                    1,
                ),
                (
                    Box::new(set_type_look_target::for_type(
                        &EntityType::PIGLIN,
                        MAX_LOOK_DIST,
                    )),
                    1,
                ),
                (
                    Box::new(set_type_look_target::for_type(
                        &EntityType::PIGLIN_BRUTE,
                        MAX_LOOK_DIST,
                    )),
                    1,
                ),
                (
                    Box::new(set_entity_look_target::create(|_, _| true, MAX_LOOK_DIST)),
                    1,
                ),
                (Box::new(DoNothing::new(30, 60)), 1),
            ])),
            Box::new(run_one(vec![
                (
                    Box::new(random_stroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                    2,
                ),
                (
                    Box::new(interact_with::of(
                        &EntityType::PIGLIN,
                        INTERACTION_RANGE,
                        types::INTERACTION_TARGET,
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        2,
                    )),
                    2,
                ),
                (
                    Box::new(interact_with::of(
                        &EntityType::PIGLIN_BRUTE,
                        INTERACTION_RANGE,
                        types::INTERACTION_TARGET,
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        2,
                    )),
                    2,
                ),
                (
                    Box::new(stroll_to_poi::create(
                        types::HOME,
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        HOME_CLOSE_ENOUGH_DISTANCE,
                        HOME_TOO_FAR_DISTANCE,
                    )),
                    2,
                ),
                (
                    Box::new(stroll_around_poi::create(
                        types::HOME,
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        HOME_STROLL_AROUND_DISTANCE,
                    )),
                    2,
                ),
                (Box::new(DoNothing::new(30, 60)), 1),
            ])),
            Box::new(set_look_and_interact::create(&EntityType::PLAYER, 4)),
        ],
    )
}

fn fight_activity() -> ActivityData {
    ActivityData::with_priority_start_and_conditions(
        Activity::Fight,
        10,
        vec![
            Box::new(stop_attacking_if_target_invalid::create(
                |tick, target| !is_nearest_valid_attack_target(tick, target),
                |_, _| {},
                true,
            )),
            Box::new(
                set_walk_target_from_attack_target_if_target_out_of_reach::create(
                    SPEED_MULTIPLIER_WHEN_FIGHTING,
                ),
            ),
            Box::new(melee_attack::create(MELEE_ATTACK_COOLDOWN)),
        ],
        vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
    )
}

fn is_nearest_valid_attack_target(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) -> bool {
    find_nearest_valid_attack_target(tick)
        .is_some_and(|nearest| nearest.get_entity().entity_id == target.get_entity().entity_id)
}

/// Vanilla `PiglinBruteAi.findNearestValidAttackTarget`: what it is angry at, else the nearest
/// visible attackable player, else the nearest visible nemesis.
fn find_nearest_valid_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let brute = tick.mob.cast_any().downcast_ref::<PiglinBruteEntity>()?;
    if let Some(angry_at) = brute
        .anger
        .angry_at()
        .and_then(|uuid| find_by_uuid(tick.world, uuid))
        .filter(|target| target.get_living_entity().is_some())
        && PiglinAi::is_entity_attackable_ignoring_line_of_sight(brute, angry_at.as_ref())
    {
        return Some(angry_at);
    }
    if let Some(player) = tick.state.get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER) {
        return Some(player.clone() as Arc<dyn EntityBase>);
    }
    tick.state.get(types::NEAREST_VISIBLE_NEMESIS).cloned()
}

const PIGLIN_BRUTE_MEMORIES: [MemoryModuleId; 1] = [types::NEAREST_VISIBLE_ADULT_PIGLINS.id()];

const PIGLIN_BRUTE_SPECIFIC_REQUIRES: [MemoryModuleId; 3] = [
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_NEMESIS.id(),
    types::NEARBY_ADULT_PIGLINS.id(),
];

/// Vanilla `PiglinBruteSpecificSensor`.
pub struct PiglinBruteSpecificSensor;

impl Sensor for PiglinBruteSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &PIGLIN_BRUTE_SPECIFIC_REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let nemesis = tick
            .state
            .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
            .and_then(|visible| {
                visible.find_closest(&tick.visibility(), |entity| {
                    let entity_type = entity.get_entity().entity_type;
                    entity_type == &EntityType::WITHER_SKELETON
                        || entity_type == &EntityType::WITHER
                })
            });
        // Vanilla `PiglinAi.findNearbyAdultPiglins`.
        let adult_piglins: Vec<_> = tick
            .state
            .get(types::NEAREST_LIVING_ENTITIES)
            .into_iter()
            .flatten()
            .filter(|entity| {
                as_abstract_piglin(entity.as_ref()).is_some_and(AbstractPiglin::is_adult_piglin)
            })
            .cloned()
            .collect();
        tick.state
            .set_optional(types::NEAREST_VISIBLE_NEMESIS, nemesis);
        tick.state.set(types::NEARBY_ADULT_PIGLINS, adult_piglins);
    }
}

impl AbstractPiglin for PiglinBruteEntity {
    fn piglin_anger(&self) -> &PiglinAnger {
        &self.anger
    }

    fn is_adult_piglin(&self) -> bool {
        true
    }

    fn can_hunt(&self) -> bool {
        false
    }

    /// Vanilla `PiglinAi.isIdle`; the brain starts out idle.
    fn is_idle(&self) -> bool {
        matches!(self.activity.load(), None | Some(Activity::Idle))
    }
}

impl Mob for PiglinBruteEntity {
    /// Vanilla `AbstractPiglin.playAmbientSound` only plays while idle.
    fn play_ambient_sound(&self) {
        if self.is_idle() {
            self.make_sound(Sound::EntityPiglinBruteAmbient);
        }
    }

    /// Vanilla `PiglinBrute.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        self.init_memories(world);
        let difficulty = RegionalDifficulty::at(world, self.get_entity().pos.load());
        self.populate_default_equipment_slots(world, &difficulty);
        self.finalize_spawn_super(group_data)
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        self.mob_entity.set_item_slot(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::GOLDEN_AXE),
        );
    }

    fn wants_to_pick_up(&self, stack: &pumpkin_data::item_stack::ItemStack) -> bool {
        stack.item.id == pumpkin_data::item::Item::GOLDEN_AXE.id && self.can_hold_item(stack)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityPiglinBruteStep, 0.15, 1.0);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        BrainProvider {
            memory_types: &PIGLIN_BRUTE_MEMORIES,
            sensor_types: &[
                SensorType::NearestLivingEntities,
                SensorType::NearestPlayers,
                SensorType::PiglinBruteSpecific,
            ],
            activities,
        }
        .make_brain(self, packed)
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        if self.is_immune_to_zombification() {
            entity.set_synced_data(
                tracked_data::abstract_piglin::DATA_IMMUNE_TO_ZOMBIFICATION,
                true,
            );
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.zombification.write_nbt(nbt);
        nbt.put_bool("CanPickUpLoot", true);
        // The anger memories live outside the brain the mob saved before this.
        let mut memories = nbt
            .get_compound("Brain")
            .and_then(|brain| brain.get_compound("memories"))
            .cloned()
            .unwrap_or_default();
        self.anger.write_memories(&mut memories);
        let mut brain = NbtCompound::new();
        brain.put_compound("memories", memories);
        nbt.put_compound("Brain", brain);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.zombification.read_nbt(&self.mob_entity, nbt);
        if let Some(memories) = nbt
            .get_compound("Brain")
            .and_then(|brain| brain.get_compound("memories"))
        {
            self.anger.read_memories(memories);
        }
    }

    /// Vanilla `PiglinBrute.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let world = self.get_entity().world.load_full();
        self.anger.tick();
        self.anger.stop_being_angry_if_target_dead(&world);
        let time = world.get_world_age();
        let (started_fighting, target) = self.mob_entity.with_brain(self, |brain| {
            brain.tick(&world, self, time);
            (
                self.update_activity(brain),
                brain.get(types::ATTACK_TARGET).cloned(),
            )
        });
        // `playActivitySound` on the change, then `maybePlayActivitySound`; both only sound in
        // FIGHT.
        if started_fighting {
            self.make_sound(Sound::EntityPiglinBruteAngry);
        }
        if game_random().random::<f32>() < ACTIVITY_SOUND_LIKELIHOOD_PER_TICK && self.is_fighting()
        {
            self.make_sound(Sound::EntityPiglinBruteAngry);
        }
        // `AbstractPiglin.getTarget` reads the brain's attack target.
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

    /// Vanilla `PiglinBrute.hurtServer` and `PiglinBruteAi.wasHurtBy`.
    fn on_damage_with_context(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(attacker) = cause.or(source)
            && attacker.get_living_entity().is_some()
            && as_abstract_piglin(attacker).is_none()
        {
            PiglinAi::maybe_retaliate(self, attacker, false);
        }
        true
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }
        self.zombification
            .tick(self, Sound::EntityPiglinBruteConvertedToZombified, || {
                PiglinZombification::finish_conversion(self);
            });
    }

    fn xp_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}
