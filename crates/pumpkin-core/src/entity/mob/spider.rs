use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::Sound;
use pumpkin_util::difficulty::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::TargetCondition, leap_at_target::LeapAtTargetGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, revenge::RevengeGoal,
        spider_attack::SpiderAttackGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    ai::pathfinder::Navigator,
    mob::{
        Mob, MobEntity, MobMetadata,
        equipment::RegionalDifficulty,
        spawn::{SpawnGroupData, finalize_spawn},
    },
    passive::armadillo::ArmadilloEntity,
    r#type::from_type,
};
use crate::world::World;

pub const SPIDER_FLEES: &[Flee] =
    &[
        Flee::new(2, TargetMatch::Type(&EntityType::ARMADILLO), 6.0, 1.0, 1.2)
            .matching(is_calm_armadillo),
    ];

/// An armadillo out of its shell.
fn is_calm_armadillo(_mob: &dyn Mob, candidate: &dyn EntityBase) -> bool {
    candidate
        .cast_any()
        .downcast_ref::<ArmadilloEntity>()
        .is_some_and(|armadillo| !armadillo.is_scared())
}

pub const SPIDER_HUNTS: &[Hunt] = &[
    Hunt::new(2, TargetMatch::Type(&EntityType::PLAYER), true).when(TargetCondition::NoDaylight),
    Hunt::new(3, TargetMatch::Type(&EntityType::IRON_GOLEM), true)
        .when(TargetCondition::NoDaylight),
];

pub struct SpiderEntity {
    pub mob_entity: MobEntity,
}

impl SpiderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        initialize_spider_navigation(&mut mob_entity);
        let mob_arc = Arc::new(Self { mob_entity });
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        register_spider_goals(&mob_arc.mob_entity, mob_weak);
        mob_arc
    }
}

/// Vanilla `Spider.createNavigation` (also used by the cave spider).
pub fn initialize_spider_navigation(mob: &mut MobEntity) {
    *mob.navigator
        .get_mut()
        .unwrap_or_else(PoisonError::into_inner) = Navigator::wall_climber();
}

/// Vanilla `Spider.registerGoals` (also used by the cave spider).
pub fn register_spider_goals(mob_entity: &MobEntity, mob_weak: Weak<dyn Mob>) {
    let goal_selector = &mob_entity.goals_selector;
    let target_selector = &mob_entity.target_selector;

    goal_selector.add_goal(1, Box::new(SwimGoal::default()));
    fleeing::apply(goal_selector, SPIDER_FLEES);
    goal_selector.add_goal(3, Box::new(LeapAtTargetGoal::new(0.4)));
    goal_selector.add_goal(4, SpiderAttackGoal::new(1.0, true));
    goal_selector.add_goal(5, Box::new(WanderAroundGoal::water_avoiding(0.8)));
    goal_selector.add_goal(
        6,
        LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
    );
    goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

    target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
    hunting::apply(target_selector, mob_entity, SPIDER_HUNTS);
}

/// Vanilla `Spider.onClimbable` reads the climbing flag, which `Spider.tick` sets from
/// `horizontalCollision`.
pub fn update_spider_climbing(mob: &MobEntity) {
    let living = &mob.living_entity;
    let entity = &living.entity;
    let climbing = entity.horizontal_collision.load(Ordering::SeqCst);
    if living.climbing.swap(climbing, Ordering::Relaxed) != climbing {
        entity.set_synced_data(
            pumpkin_data::tracked_data::spider::DATA_FLAGS_ID,
            i8::from(climbing),
        );
        entity.send_bedrock_actor_data(&spider_metadata(mob).to_bedrock_actor_data());
    }
}

pub fn spider_metadata(mob: &MobEntity) -> MobMetadata {
    let meta = MobMetadata::new();
    if mob.living_entity.climbing.load(Ordering::Relaxed) {
        meta.spider_climbing(true)
    } else {
        meta
    }
}

/// Vanilla `Spider.finalizeSpawn` (also used by the cave spider): a skeleton jockey,
/// and on hard difficulty a random effect shared by the whole spawn group.
pub fn finalize_spider_spawn(
    mob: &MobEntity,
    world: &Arc<World>,
    reason: crate::entity::mob::spawn::EntitySpawnReason,
    group_data: Option<SpawnGroupData>,
) -> Option<SpawnGroupData> {
    mob.finalize_spawn_base();
    let rng = game_random();
    let entity = &mob.living_entity.entity;
    let pos = entity.pos.load();

    if rng.random_range(0..100) == 0 {
        let skeleton = from_type(&EntityType::SKELETON, pos, world, Uuid::new_v4());
        skeleton.get_entity().set_rotation(entity.yaw.load(), 0.0);
        finalize_spawn(&skeleton, world, reason, None);
        mob.add_pending_rider(skeleton);
    }

    let group_data = group_data.unwrap_or_else(|| {
        let difficulty = RegionalDifficulty::at(world, pos);
        let effect = (difficulty.base_difficulty == Difficulty::Hard
            && rng.random::<f32>() < 0.1 * difficulty.special_multiplier)
            .then(|| match rng.random_range(0..5) {
                0 | 1 => &StatusEffect::SPEED,
                2 => &StatusEffect::STRENGTH,
                3 => &StatusEffect::REGENERATION,
                _ => &StatusEffect::INVISIBILITY,
            });
        SpawnGroupData::SpiderEffects(effect)
    });

    if let SpawnGroupData::SpiderEffects(Some(effect_type)) = &group_data {
        mob.living_entity.add_effect(Effect {
            effect_type,
            duration: -1,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }
    Some(group_data)
}

/// Vanilla `Spider.getVehicleAttachmentPoint` and `CaveSpider.getVehicleAttachmentPoint`: a
/// spider sits `height` (times its scale) above a vehicle no wider than itself.
pub fn spider_vehicle_attachment_point(
    mob_entity: &MobEntity,
    vehicle: &dyn EntityBase,
    height: f64,
) -> Vector3<f64> {
    let entity = &mob_entity.living_entity.entity;
    let width = |entity: &Entity| entity.get_dimensions(entity.pose.load()).width;
    if width(vehicle.get_entity()) <= width(entity) {
        Vector3::new(0.0, height * entity.attachment_scale(), 0.0)
    } else {
        entity.default_vehicle_attachment_point()
    }
}

impl Mob for SpiderEntity {
    fn mob_vehicle_attachment_point(&self, vehicle: &dyn EntityBase) -> Vector3<f64> {
        spider_vehicle_attachment_point(&self.mob_entity, vehicle, 0.3125)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntitySpiderStep, 0.15, 1.0);
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        finalize_spider_spawn(&self.mob_entity, world, reason, group_data)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_metadata(&self) -> Option<MobMetadata> {
        Some(spider_metadata(&self.mob_entity))
    }

    fn post_tick(&self) {
        update_spider_climbing(&self.mob_entity);
    }
}
