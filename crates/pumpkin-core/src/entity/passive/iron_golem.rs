use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;

use crate::enchantment::EnchantmentHelper;
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::{
    Entity, EntityBase,
    ai::behavior::neutral::apply_targets,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, move_towards_target::MoveTowardsTargetGoal,
        offer_flower::OfferFlowerGoal, revenge::RevengeGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    player::Player,
};
use crate::world::natural_spawner::is_valid_empty_spawn_block;

/// Represents an Iron Golem, a powerful neutral mob that protects villagers and players.
///
/// Wiki: <https://minecraft.wiki/w/Iron_Golem>
const IRON_GOLEM_HUNTS: &[Hunt] = &[Hunt::new(3, TargetMatch::Enemy, false)
    .every(5)
    .matching(|_hunter, target, _world| target.entity.entity_type != &EntityType::CREEPER)];

pub struct IronGolemEntity {
    pub mob_entity: MobEntity,
    pub neutral_data: NeutralData,
    pub player_created: AtomicBool,
    pub attack_animation_tick: AtomicI32,
    pub offer_flower_tick: AtomicI32,
}

impl IronGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let iron_golem = Self {
            mob_entity,
            neutral_data: NeutralData::default(),
            player_created: AtomicBool::new(false),
            attack_animation_tick: AtomicI32::new(0),
            offer_flower_tick: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(iron_golem);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(1, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(2, Box::new(MoveTowardsTargetGoal::new(0.9, 32.0)));
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(5, Box::new(OfferFlowerGoal::new()));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            // TODO: DefendVillageTargetGoal at 1.
            target_selector.add_goal(2, Box::new(RevengeGoal::new(true)));
            apply_targets(target_selector, &mob_arc.mob_entity, 3, 4, false);
            hunting::apply(target_selector, &mob_arc.mob_entity, IRON_GOLEM_HUNTS);
        };

        mob_arc
    }

    #[must_use]
    pub fn is_player_created(&self) -> bool {
        self.player_created.load(Ordering::Relaxed)
    }

    pub fn set_player_created(&self, value: bool) {
        self.player_created.store(value, Ordering::Relaxed);
        let entity = self.get_entity();
        let flag: u8 = u8::from(value);
        entity.set_synced_data(pumpkin_data::tracked_data::iron_golem::FLAGS_ID, flag);
    }

    pub fn offer_flower(&self, offer: bool) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if offer {
            self.offer_flower_tick.store(400, Ordering::Relaxed);
            world.send_entity_status(
                entity,
                EntityStatus::OfferFlower,
                ActorEventID::StartOfferFlower,
            );
        } else {
            self.offer_flower_tick.store(0, Ordering::Relaxed);
            world.send_entity_status(
                entity,
                EntityStatus::StopOfferFlower,
                ActorEventID::StopOfferFlower,
            );
        }
    }

    #[must_use]
    pub fn get_offer_flower_tick(&self) -> i32 {
        self.offer_flower_tick.load(Ordering::Relaxed)
    }

    /// Trigger the attack animation for the iron golem.
    /// This sends `EntityStatus::StartAttacking` to clients and sets the `attack_animation_tick` counter.
    pub fn trigger_attack_animation(&self) {
        self.attack_animation_tick.store(10, Ordering::Relaxed);
        let entity = self.get_entity();
        let world = entity.world.load();
        world.send_entity_status(
            entity,
            EntityStatus::StartAttacking,
            ActorEventID::StartAttacking,
        );
    }
}

impl NeutralMob for IronGolemEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }
}

impl Mob for IronGolemEntity {
    /// Vanilla `IronGolem.checkSpawnObstruction`: solid footing and room for its height.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        let entity = self.get_entity();
        let pos = entity.block_pos.load();
        if !crate::entity::spawn_util::is_face_full_up(level.block_state(&pos.down())) {
            return false;
        }
        if !(1..3).all(|height| {
            is_valid_empty_spawn_block(
                level.block_state(&pos.up_height(height)),
                entity.entity_type,
            )
        }) {
            return false;
        }
        // Vanilla checks its own block with an empty fluid, so a golem can stand in water.
        let state = level.block_state(&pos);
        (state.is_liquid() || is_valid_empty_spawn_block(state, entity.entity_type))
            && level
                .entities_at_box(&entity.bounding_box.load())
                .iter()
                .all(|other| other.get_living_entity().is_none() || other.get_entity().is_removed())
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        self.play_entity_sound(Sound::EntityIronGolemStep, 1.0, 1.0);
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        let target_type = target.get_entity().entity_type;
        if self.is_player_created() && target_type == &EntityType::PLAYER {
            return false;
        }
        // Vanilla's `super.canAttack` goes through `Mob` crepper and ghast
        target_type != &EntityType::CREEPER
            && target_type != &EntityType::GHAST
            && self.get_mob_entity().living_entity.can_attack(target)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("PlayerCreated", self.is_player_created());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(created) = nbt.get_bool("PlayerCreated") {
            self.set_player_created(created);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_iron_golem(&self) -> Option<&crate::entity::passive::iron_golem::IronGolemEntity> {
        Some(self)
    }

    /// Vanilla `IronGolem.doHurtTarget`: a random hit that throws the target upwards.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        self.trigger_attack_animation();
        let attack_damage = self
            .mob_entity
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        let damage = if attack_damage as i32 > 0 {
            attack_damage / 2.0 + game_random().random_range(0..attack_damage as i32) as f32
        } else {
            attack_damage
        };
        let hurt = target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK,
            None,
            Some(self),
            Some(self),
        );
        if hurt {
            let knockback_resistance = target.get_living_entity().map_or(0.0, |living| {
                living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
            });
            let scale = (1.0 - knockback_resistance).max(0.0);
            target
                .get_entity()
                .add_velocity(Vector3::new(0.0, f64::from(0.4f32) * scale, 0.0));
            EnchantmentHelper::do_post_attack_effects(
                target,
                DamageType::MOB_ATTACK,
                Some(self),
                Some(self),
            );
        }
        self.play_entity_sound(Sound::EntityIronGolemAttack, 1.0, 1.0);
        hurt
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let attack_tick = self.attack_animation_tick.load(Ordering::Relaxed);
        if attack_tick > 0 {
            self.attack_animation_tick.fetch_sub(1, Ordering::Relaxed);
        }

        let flower_tick = self.offer_flower_tick.load(Ordering::Relaxed);
        if flower_tick > 0 {
            self.offer_flower_tick.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flag: u8 = u8::from(self.is_player_created());
        entity.set_synced_data(pumpkin_data::tracked_data::iron_golem::FLAGS_ID, flag);
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.item.id == Item::IRON_INGOT.id {
            let living = &self.mob_entity.living_entity;
            let current_health = living.health.load();
            let max_health = living.get_max_health();
            if current_health < max_health {
                living.set_health((current_health + 25.0).min(max_health));
                let entity = self.get_entity();
                let world = entity.world.load();
                let pos = entity.pos.load();
                world.play_sound(Sound::EntityIronGolemRepair, SoundCategory::Neutral, &pos);
                if player.gamemode.load() != GameMode::Creative {
                    item_stack.item_count = item_stack.item_count.saturating_sub(1);
                }
                return true;
            }
        }
        false
    }
}
