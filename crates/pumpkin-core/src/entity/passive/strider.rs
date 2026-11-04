use crate::entity::r#type::CreatureSpawnLevel;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::BlockState;
use pumpkin_data::block_properties::WaterLikeProperties;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::block::PathComputationType;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        Controls, Goal, ParentHandle,
        breed::BreedGoal,
        escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal},
        tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    ai::pathfinder::{GroundPathNavigation, Navigator, node::PathType},
    item_steerable::{ItemBasedSteering, ItemSteerable},
    mob::{Mob, MobEntity, equipment::equip_on_target},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

const TEMPT_ITEMS: &[&Item] = &[&Item::WARPED_FUNGUS, &Item::WARPED_FUNGUS_ON_A_STICK];
/// Vanilla `Strider.getLiquidCollisionShape`, `Block.column(16.0, 0.0, 8.0)`.
const LIQUID_COLLISION_SHAPE: BoundingBox =
    BoundingBox::new_array([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]);

fn is_lava(world: &World, pos: &BlockPos) -> bool {
    world.get_fluid(pos).has_tag(&tag::Fluid::MINECRAFT_LAVA)
}

/// Vanilla `LiquidBlock.getCollisionShape` for a strider, the only mob whose `canStandOnFluid`
/// accepts lava: a lava source it stands above carries it unless more lava is on top.
pub(crate) fn lava_collision_shape_for_entity(
    entity: &dyn EntityBase,
    state: &BlockState,
    pos: &BlockPos,
) -> Option<BoundingBox> {
    let base = entity.get_entity();
    (base.entity_type == &EntityType::STRIDER
        && WaterLikeProperties::from_state_id(state.id).level == 0
        && World::is_entity_above(base, pos, LIQUID_COLLISION_SHAPE.max.y)
        && !is_lava(&base.world.load(), &pos.up()))
    .then_some(LIQUID_COLLISION_SHAPE)
}

/// Vanilla `Strider.StriderGoToLavaGoal`: a strider out of lava heads for a lava block nearby.
struct StriderGoToLavaGoal {
    move_to_block: MoveToTargetPosGoal<Self>,
}

impl StriderGoToLavaGoal {
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the inner goal points back into this heap allocation"
    )]
    fn new(speed: f64) -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_block: MoveToTargetPosGoal::new(ParentHandle::none(), speed, 8, 2),
        });
        // SAFETY: the boxed goal never moves, so the handle stays valid for the goal's lifetime.
        this.move_to_block.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }
}

impl Goal for StriderGoToLavaGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_entity().touching_lava.load(Ordering::Relaxed) && self.move_to_block.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        !entity.touching_lava.load(Ordering::Relaxed)
            && self.is_target_pos(entity.world.load_full(), self.move_to_block.target_pos)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_block.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_block.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_block.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_block.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_block.controls()
    }
}

impl MoveToTargetPos for StriderGoToLavaGoal {
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        let above = block_pos.up();
        let (above_block, above_state) = world.get_block_and_state(&above);
        world.get_block(&block_pos).id == pumpkin_data::Block::LAVA.id
            && world.block_registry.is_pathfindable(
                above_block,
                above_state,
                PathComputationType::Land,
            )
    }

    fn get_move_to_target(&self, target_pos: BlockPos) -> BlockPos {
        target_pos
    }

    fn should_recalculate_path(&self, trying_time: i32) -> bool {
        trying_time % 20 == 0
    }
}

pub struct StriderEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub steering: ItemBasedSteering,
    pub suffocating: AtomicBool,
}

impl StriderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let strider = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            steering: ItemBasedSteering::default(),
            suffocating: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(strider);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        let mut navigator = mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::new(GroundPathNavigation::strider());
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        navigator.set_pathfinding_malus(PathType::Lava, 0.0);
        navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
        navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
        drop(navigator);

        {
            // Vanilla `Strider.registerGoals`: no float goal, it stands on lava.
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            goal_selector.add_goal(1, EscapeDangerGoal::new(1.65));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.4, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, StriderGoToLavaGoal::new(1.0));
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::with_interval(1.0, 60)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::STRIDER, 8.0),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn is_suffocating(&self) -> bool {
        self.suffocating.load(Ordering::Relaxed)
    }

    pub fn set_suffocating(&self, suffocating: bool) {
        self.suffocating.store(suffocating, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::strider::DATA_SUFFOCATING,
            suffocating,
        );
    }
}

impl AgeableMob for StriderEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn default_baby_spawn_chance(&self) -> f32 {
        crate::entity::ageable::STRIDER_BABY_SPAWN_CHANCE
    }
}

impl Animal for StriderEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_STRIDER_FOOD)
            || item_stack.item == &Item::WARPED_FUNGUS
    }
}

impl Mob for StriderEntity {
    /// Vanilla `Strider.checkSpawnObstruction`.
    fn check_spawn_obstruction(&self, level: &dyn CreatureSpawnLevel) -> bool {
        crate::entity::mob::is_unobstructed(self, level)
    }

    /// Vanilla `Strider.getWalkTargetValue`.
    fn get_walk_target_value(&self, level: &dyn CreatureSpawnLevel, pos: &BlockPos) -> f32 {
        if level.fluid(pos).has_tag(&tag::Fluid::MINECRAFT_LAVA) {
            10.0
        } else if self.get_entity().touching_lava.load(Ordering::Relaxed) {
            f32::NEG_INFINITY
        } else {
            0.0
        }
    }

    fn can_dispenser_equip_into_slot(&self, slot: &EquipmentSlot) -> bool {
        *slot == EquipmentSlot::SADDLE || self.mob_entity.can_pick_up_loot()
    }

    fn mob_controlling_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        self.saddled_controlling_passenger(Some(&Item::WARPED_FUNGUS_ON_A_STICK))
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        let sound = if self.get_entity().touching_lava.load(Ordering::Relaxed) {
            Sound::EntityStriderStepLava
        } else {
            Sound::EntityStriderStep
        };
        self.play_entity_sound(sound, 1.0, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        self.mob_entity
            .read_legacy_saddle(nbt.get_bool("Saddle") == Some(true));
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    /// Vanilla `Strider.floatStrider`, run after the rest of the tick.
    fn post_tick(&self) {
        let entity = self.get_entity();
        if !entity.touching_lava.load(Ordering::Relaxed) {
            return;
        }
        let block_pos = entity.block_pos.load();
        if World::is_entity_above(entity, &block_pos, LIQUID_COLLISION_SHAPE.max.y)
            && !is_lava(&entity.world.load(), &block_pos.up())
        {
            entity.on_ground.store(true, Ordering::Relaxed);
        } else {
            entity
                .velocity
                .store(entity.velocity.load() * 0.5 + Vector3::new(0.0, 0.05, 0.0));
        }
    }

    /// Vanilla `Strider.checkFallDamage`: lava resets the fall instead of landing on it.
    fn should_check_fall_damage(&self) -> bool {
        if self.get_entity().touching_lava.load(Ordering::Relaxed) {
            self.mob_entity.living_entity.fall_distance.store(0.0);
            return false;
        }
        true
    }

    fn get_item_steerable(&self) -> Option<&dyn ItemSteerable> {
        Some(self)
    }

    fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        *slot != EquipmentSlot::SADDLE
            || (self.mob_entity.living_entity.entity.is_alive() && !self.is_baby())
    }

    fn get_equip_sound(&self, slot: &EquipmentSlot) -> Option<Sound> {
        (*slot == EquipmentSlot::SADDLE).then_some(Sound::EntityStriderSaddle)
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::strider::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::strider::DATA_SUFFOCATING,
            self.is_suffocating(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if !self.is_food(item_stack)
            && self.is_saddled()
            && !self.get_entity().has_passengers()
            && !player.get_entity().is_sneaking()
        {
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
            }
            return true;
        }

        if self.animal_interact(player, item_stack) {
            return true;
        }
        self.is_equippable_in_slot(item_stack, &EquipmentSlot::SADDLE)
            && equip_on_target(self, player, item_stack)
    }
}

impl ItemSteerable for StriderEntity {
    fn boost(&self) -> bool {
        self.steering.boost()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
