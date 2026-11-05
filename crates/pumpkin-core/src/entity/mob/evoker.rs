use std::sync::{Arc, Weak};
use uuid::Uuid;

use pumpkin_data::BlockDirection;
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::spellcaster::{
    Spell, SpellTarget, SpellcasterCastingSpellGoal, SpellcasterState, SpellcasterUseSpellGoal,
};
use crate::entity::passive::sheep::SheepEntity;
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
        spawn::{EntitySpawnReason, finalize_spawn},
        vex::VexEntity,
    },
    projectile::evoker_fangs::EvokerFangsEntity,
    r#type::from_type,
};
use crate::world::game_event::GameEventContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IllagerSpell {
    None = 0,
    SummonVex = 1,
    Fangs = 2,
    Wololo = 3,
    Disappear = 4,
    Blindness = 5,
}

impl IllagerSpell {
    #[must_use]
    pub const fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::SummonVex,
            2 => Self::Fangs,
            3 => Self::Wololo,
            4 => Self::Disappear,
            5 => Self::Blindness,
            _ => Self::None,
        }
    }
}

const EVOKER_FLEES: &[Flee] = &[
    Flee::new(2, TargetMatch::Type(&EntityType::PLAYER), 8.0, 0.6, 1.0),
    raider::flee_creaking(3, 0.6, 1.0),
];

const EVOKER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::VILLAGER_LIKE, false).forget_unseen_after(300),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), false),
];

pub struct EvokerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    spellcaster: Arc<SpellcasterState>,
}

impl EvokerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `SpellcasterIllager.defineSynchedData`, so the default isn't sent.
        entity
            .synched_data
            .define(tracked_data::evoker::DATA_SPELL_CASTING_ID, 0i8);
        let mob_entity = MobEntity::new(entity);
        let evoker = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            spellcaster: Arc::new(SpellcasterState::new(
                tracked_data::evoker::DATA_SPELL_CASTING_ID,
                Sound::EntityEvokerCastSpell,
            )),
        };
        let mob_arc = Arc::new(evoker);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let spellcaster = &mob_arc.spellcaster;
            let wololo_target = SpellTarget::default();

            // `PatrollingMonster` and `Raider` register theirs first.
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                Box::new(
                    SpellcasterCastingSpellGoal::new(spellcaster.clone())
                        .looking_at_without_target(wololo_target.clone()),
                ),
            );
            fleeing::apply(goal_selector, EVOKER_FLEES);
            goal_selector.add_goal(
                4,
                Box::new(SpellcasterUseSpellGoal::new(
                    spellcaster.clone(),
                    EvokerSummonSpell::new(),
                )),
            );
            goal_selector.add_goal(
                5,
                Box::new(SpellcasterUseSpellGoal::new(
                    spellcaster.clone(),
                    EvokerAttackSpell,
                )),
            );
            goal_selector.add_goal(
                6,
                Box::new(SpellcasterUseSpellGoal::new(
                    spellcaster.clone(),
                    EvokerWololoSpell::new(wololo_target),
                )),
            );
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
            hunting::apply(target_selector, &mob_arc.mob_entity, EVOKER_HUNTS);
        };

        mob_arc
    }
}

impl Mob for EvokerEntity {
    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        self.spellcaster.write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        self.spellcaster.read_nbt(nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.spellcaster.tick();
    }
}

impl PatrollingMonster for EvokerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for EvokerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityEvokerCelebrate
    }
}

/// Vanilla `Evoker.EvokerAttackSpellGoal`: a row or two rings of fangs.
struct EvokerAttackSpell;

impl EvokerAttackSpell {
    /// Vanilla `createSpellEntity`: fangs on the first sturdy floor from `max_y` down to one
    /// block below `min_y`.
    fn create_spell_entity(
        mob: &dyn Mob,
        x: f64,
        z: f64,
        min_y: f64,
        max_y: f64,
        angle: f32,
        delay_ticks: u32,
    ) {
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        let mut pos = BlockPos::floored(x, max_y, z);
        let mut top_offset = 0.0;
        let mut success = false;
        loop {
            let below = pos.down();
            if world
                .get_block_state(&below)
                .is_side_solid(BlockDirection::Up)
            {
                let state = world.get_block_state(&pos);
                if !state.is_air()
                    && let Some(top) = state
                        .get_block_collision_shapes_at(&pos)
                        .map(|shape| shape.max.y)
                        .reduce(f64::max)
                {
                    top_offset = top;
                }
                success = true;
                break;
            }
            pos = below;
            if pos.0.y < min_y.floor() as i32 - 1 {
                break;
            }
        }

        if success {
            let fangs_pos = Vector3::new(x, f64::from(pos.0.y) + top_offset, z);
            let fangs = Arc::new(EvokerFangsEntity::new(
                Entity::from_uuid(
                    Uuid::new_v4(),
                    world.clone(),
                    fangs_pos,
                    &EntityType::EVOKER_FANGS,
                ),
                delay_ticks,
                angle.to_degrees(),
                Some(entity),
            ));
            world.spawn_entity(fangs);
            world.emit_game_event_with_context(
                GameEvent::EntityPlace.name(),
                fangs_pos,
                &GameEventContext {
                    source_entity: Some(mob),
                },
            );
        }
    }
}

impl Spell for EvokerAttackSpell {
    fn casting_time(&self) -> i32 {
        40
    }

    fn casting_interval(&self) -> i32 {
        100
    }

    fn prepare_sound(&self) -> Option<Sound> {
        Some(Sound::EntityEvokerPrepareAttack)
    }

    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Fangs
    }

    fn perform_spell_casting(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let pos = mob.get_entity().pos.load();
        let target_pos = target.get_entity().pos.load();
        let min_y = target_pos.y.min(pos.y);
        let max_y = target_pos.y.max(pos.y) + 1.0;
        let angle_towards_target = (target_pos.z - pos.z).atan2(target_pos.x - pos.x) as f32;
        let fang = |angle: f32, distance: f64, delay_ticks: u32| {
            Self::create_spell_entity(
                mob,
                f64::from(pumpkin_util::math::cos(angle)).mul_add(distance, pos.x),
                f64::from(pumpkin_util::math::sin(angle)).mul_add(distance, pos.z),
                min_y,
                max_y,
                angle,
                delay_ticks,
            );
        };
        if pos.squared_distance_to_vec(&target_pos) < 9.0 {
            for i in 0..5 {
                fang(
                    angle_towards_target + i as f32 * std::f32::consts::PI * 0.4,
                    1.5,
                    0,
                );
            }
            for i in 0..8 {
                fang(
                    angle_towards_target
                        + i as f32 * std::f32::consts::PI * 2.0 / 8.0
                        + (std::f64::consts::PI * 2.0 / 5.0) as f32,
                    2.5,
                    3,
                );
            }
        } else {
            for i in 0..16u32 {
                fang(angle_towards_target, 1.25 * f64::from(i + 1), i);
            }
        }
    }
}

/// Vanilla `Evoker.EvokerSummonSpellGoal`: three vexes, unless enough are already near.
struct EvokerSummonSpell {
    vex_count_targeting: TargetPredicate,
}

impl EvokerSummonSpell {
    fn new() -> Self {
        Self {
            vex_count_targeting: TargetPredicate::create_non_attackable()
                .set_base_max_distance(16.0)
                .ignore_visibility()
                .ignore_distance_scaling_factor(),
        }
    }
}

impl Spell for EvokerSummonSpell {
    fn can_use(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let bounding_box = entity.bounding_box.load().expand(16.0, 16.0, 16.0);
        let vexes = world
            .get_entities_of_types_at_box(&bounding_box, |entity_type| {
                entity_type == &EntityType::VEX
            })
            .iter()
            .filter(|vex| {
                self.vex_count_targeting
                    .test(&world, Some(mob), vex.as_ref())
            })
            .count();
        mob.get_random().random_range(0..8) + 1 > vexes
    }

    fn casting_time(&self) -> i32 {
        100
    }

    fn casting_interval(&self) -> i32 {
        340
    }

    fn prepare_sound(&self) -> Option<Sound> {
        Some(Sound::EntityEvokerPrepareSummon)
    }

    fn spell(&self) -> IllagerSpell {
        IllagerSpell::SummonVex
    }

    fn perform_spell_casting(&mut self, mob: &dyn Mob) {
        let evoker = mob.get_entity();
        let world = evoker.world.load_full();
        let evoker_pos = evoker.block_pos.load();
        let evoker_team = mob.get_team_name();

        for _ in 0..3 {
            let rng = mob.get_random();
            let x = -2 + rng.random_range(0..5);
            let z = -2 + rng.random_range(0..5);
            let pos = evoker_pos.offset(Vector3::new(x, 1, z));
            let spawn_pos = Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y),
                f64::from(pos.0.z) + 0.5,
            );
            let vex = from_type(&EntityType::VEX, spawn_pos, &world, Uuid::new_v4());
            finalize_spawn(&vex, &world, EntitySpawnReason::MobSummoned, None);
            if let Some(vex) = vex.cast_any().downcast_ref::<VexEntity>() {
                vex.set_owner(evoker);
                vex.set_bound_origin(Some(pos));
                vex.set_limited_life(20 * (30 + rng.random_range(0..90)));
            }
            if let Some(team) = &evoker_team {
                vex.get_entity().join_team(team);
            }
            world.spawn_entity(vex);
            world.emit_game_event_with_context(
                GameEvent::EntityPlace.name(),
                pos.to_centered_f64(),
                &GameEventContext {
                    source_entity: Some(mob),
                },
            );
        }
    }
}

/// Vanilla `Evoker.EvokerWololoSpellGoal`: turns a blue sheep red while there is no target.
struct EvokerWololoSpell {
    wololo_target: SpellTarget,
    wololo_targeting: TargetPredicate,
}

impl EvokerWololoSpell {
    fn new(wololo_target: SpellTarget) -> Self {
        Self {
            wololo_target,
            wololo_targeting: TargetPredicate::create_non_attackable().set_base_max_distance(16.0),
        }
    }

    fn target(&self) -> std::sync::MutexGuard<'_, Option<Arc<dyn EntityBase>>> {
        self.wololo_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn is_blue_sheep(entity: &dyn EntityBase) -> bool {
    entity
        .cast_any()
        .downcast_ref::<SheepEntity>()
        .is_some_and(|sheep| sheep.get_color() == DyeColor::Blue as u8)
}

impl Spell for EvokerWololoSpell {
    fn needs_target(&self) -> bool {
        false
    }

    fn can_use(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        if !world.level_info.load().game_rules.mob_griefing {
            return false;
        }
        let bounding_box = entity.bounding_box.load().expand(16.0, 4.0, 16.0);
        let sheep: Vec<_> = world
            .get_entities_of_types_at_box(&bounding_box, |entity_type| {
                entity_type == &EntityType::SHEEP
            })
            .into_iter()
            .filter(|sheep| {
                is_blue_sheep(sheep.as_ref())
                    && self
                        .wololo_targeting
                        .test(&world, Some(mob), sheep.as_ref())
            })
            .collect();
        if sheep.is_empty() {
            return false;
        }
        let index = mob.get_random().random_range(0..sheep.len());
        *self.target() = Some(sheep[index].clone());
        true
    }

    fn can_continue(&self, _mob: &dyn Mob) -> bool {
        self.target().is_some()
    }

    fn on_stop(&mut self, _mob: &dyn Mob) {
        *self.target() = None;
    }

    fn cast_warmup_time(&self) -> i32 {
        40
    }

    fn casting_time(&self) -> i32 {
        60
    }

    fn casting_interval(&self) -> i32 {
        140
    }

    fn prepare_sound(&self) -> Option<Sound> {
        Some(Sound::EntityEvokerPrepareWololo)
    }

    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Wololo
    }

    fn perform_spell_casting(&mut self, _mob: &dyn Mob) {
        if let Some(target) = self.target().as_ref()
            && target.get_entity().is_alive()
            && let Some(sheep) = target.cast_any().downcast_ref::<SheepEntity>()
        {
            sheep.set_color(DyeColor::Red as u8);
        }
    }
}
