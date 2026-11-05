use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::goal::bow_attack::BowAttackGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::evoker::IllagerSpell;
use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData};
use crate::entity::mob::spellcaster::{
    Spell, SpellcasterCastingSpellGoal, SpellcasterState, SpellcasterUseSpellGoal,
};
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{look_at_entity::LookAtEntityGoal, swim::SwimGoal, wander_around::WanderAroundGoal},
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            self, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider, RaiderCelebrationGoal,
            RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};
use crate::world::World;

const ILLUSIONER_FLEES: &[Flee] = &[raider::flee_creaking(3, 1.0, 1.2)];

const ILLUSIONER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), false).forget_unseen_after(300),
];

/// Vanilla `Difficulty.NORMAL.ordinal()`, which the blindness spell's local difficulty must pass.
const BLINDNESS_MIN_DIFFICULTY: f32 = 2.0;
const BLINDNESS_DURATION: i32 = 400;
const INVISIBILITY_DURATION: i32 = 1200;

pub struct IllusionerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    spellcaster: Arc<SpellcasterState>,
}

impl IllusionerEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `SpellcasterIllager.defineSynchedData`, so the default isn't sent.
        entity
            .synched_data
            .define(tracked_data::illusioner::DATA_SPELL_CASTING_ID, 0i8);
        let mob_entity = MobEntity::new(entity);
        let illusioner = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            spellcaster: Arc::new(SpellcasterState::new(
                tracked_data::illusioner::DATA_SPELL_CASTING_ID,
                Sound::EntityIllusionerCastSpell,
            )),
        };
        let mob_arc = Arc::new(illusioner);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let spellcaster = &mob_arc.spellcaster;

            // `PatrollingMonster` and `Raider` register theirs first.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                Box::new(SpellcasterCastingSpellGoal::new(spellcaster.clone())),
            );
            fleeing::apply(goal_selector, ILLUSIONER_FLEES);
            goal_selector.add_goal(
                4,
                Box::new(SpellcasterUseSpellGoal::new(
                    spellcaster.clone(),
                    IllusionerMirrorSpell,
                )),
            );
            goal_selector.add_goal(
                5,
                Box::new(SpellcasterUseSpellGoal::new(
                    spellcaster.clone(),
                    IllusionerBlindnessSpell { last_target_id: 0 },
                )),
            );
            goal_selector.add_goal(6, Box::new(BowAttackGoal::new(0.5, 20, 15.0)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
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
            hunting::apply(target_selector, &mob_arc.mob_entity, ILLUSIONER_HUNTS);
        };

        mob_arc
    }
}

impl Mob for IllusionerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.spellcaster.tick();
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::BOW));
        self.finalize_spawn_super(group_data)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        self.spellcaster.write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        self.spellcaster.read_nbt(nbt);
    }
}

impl PatrollingMonster for IllusionerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for IllusionerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityIllusionerAmbient
    }
}

/// Vanilla `Illusioner.IllusionerMirrorSpellGoal`: turns invisible.
struct IllusionerMirrorSpell;

impl Spell for IllusionerMirrorSpell {
    fn can_use(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_mob_entity()
            .living_entity
            .has_effect(&StatusEffect::INVISIBILITY)
    }

    fn casting_time(&self) -> i32 {
        20
    }

    fn casting_interval(&self) -> i32 {
        340
    }

    fn prepare_sound(&self) -> Option<Sound> {
        Some(Sound::EntityIllusionerPrepareMirror)
    }

    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Disappear
    }

    fn perform_spell_casting(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().living_entity.add_effect(plain_effect(
            &StatusEffect::INVISIBILITY,
            INVISIBILITY_DURATION,
        ));
    }
}

/// Vanilla `Illusioner.IllusionerBlindnessSpellGoal`: blinds each new target once, on a hard
/// enough local difficulty.
struct IllusionerBlindnessSpell {
    last_target_id: i32,
}

impl Spell for IllusionerBlindnessSpell {
    fn can_use(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        if target.get_entity().entity_id == self.last_target_id {
            return false;
        }
        let entity = mob.get_entity();
        RegionalDifficulty::at(&entity.world.load_full(), entity.pos.load()).effective_difficulty
            > BLINDNESS_MIN_DIFFICULTY
    }

    fn on_start(&mut self, mob: &dyn Mob) {
        if let Some(target) = mob.get_mob_entity().get_target() {
            self.last_target_id = target.get_entity().entity_id;
        }
    }

    fn casting_time(&self) -> i32 {
        20
    }

    fn casting_interval(&self) -> i32 {
        180
    }

    fn prepare_sound(&self) -> Option<Sound> {
        Some(Sound::EntityIllusionerPrepareBlindness)
    }

    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Blindness
    }

    fn perform_spell_casting(&mut self, mob: &dyn Mob) {
        if let Some(target) = mob.get_mob_entity().get_target()
            && let Some(living) = target.get_living_entity()
        {
            living.add_effect_with_source(
                plain_effect(&StatusEffect::BLINDNESS, BLINDNESS_DURATION),
                Some(mob),
            );
        }
    }
}

/// Vanilla `new MobEffectInstance(effect, duration)`.
const fn plain_effect(effect_type: &'static StatusEffect, duration: i32) -> Effect {
    Effect {
        effect_type,
        duration,
        amplifier: 0,
        ambient: false,
        show_particles: true,
        show_icon: true,
        blend: false,
    }
}
