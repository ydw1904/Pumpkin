use std::sync::{Arc, Weak};

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::util::land_random_pos;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, open_door::OpenDoorGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        equipment::RegionalDifficulty,
        piglin_ai::{
            AbstractPiglin, PiglinAi, PiglinAnger, PiglinAngerTargetGoal, PiglinZombification,
            as_abstract_piglin,
        },
    },
};
use crate::world::World;

/// Vanilla `PiglinBruteAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f64 = 0.6;

pub struct PiglinBruteEntity {
    pub mob_entity: MobEntity,
    pub zombification: PiglinZombification,
    pub anger: PiglinAnger,
}

impl PiglinBruteEntity {
    pub const XP_REWARD: u32 = 20;

    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `AbstractPiglin.defineSynchedData`.
        crate::entity::mob::piglin::define_abstract_piglin_synched_data(&entity);
        let mut mob_entity = MobEntity::new(entity);
        crate::entity::mob::piglin::apply_abstract_piglin_navigation(&mut mob_entity);
        let piglin = Self {
            mob_entity,
            zombification: PiglinZombification::default(),
            anger: PiglinAnger::default(),
        };
        let mob_arc = Arc::new(piglin);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(OpenDoorGoal::new(true)));
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(1.0, true)));
            // Vanilla `RandomStroll.stroll(0.6)`, which ignores the no-action time.
            goal_selector.add_goal(
                5,
                Box::new(
                    WanderAroundGoal::new(SPEED_MULTIPLIER_WHEN_IDLING)
                        .with_check_no_action_time(false)
                        .with_position(|mob| land_random_pos::get_pos(mob, 10, 7), 0.0),
                ),
            );
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let target_selector = &mob_arc.mob_entity.target_selector;
            target_selector.add_goal(1, Box::new(PiglinAngerTargetGoal::default()));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, TargetMatch::WITHER_LIKE, true),
            );
        };

        mob_arc
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

    // A brute's only other activity is fighting its target.
    fn is_idle(&self) -> bool {
        self.mob_entity.get_target().is_none()
    }
}

impl Mob for PiglinBruteEntity {
    // Vanilla `AbstractPiglin.playAmbientSound` only plays while idle, and a brute's only other
    // activity is fighting its target.
    fn play_ambient_sound(&self) {
        if self.mob_entity.get_target().is_none() {
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
        let mut memories = NbtCompound::new();
        self.anger.write_memories(&mut memories);
        if !memories.child_tags.is_empty() {
            let mut brain = NbtCompound::new();
            brain.put("memories", NbtTag::Compound(memories));
            nbt.put("Brain", NbtTag::Compound(brain));
        }
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

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.anger.tick();
        self.anger
            .stop_being_angry_if_target_dead(&self.get_entity().world.load());
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
