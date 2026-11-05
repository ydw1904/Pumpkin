mod goals;

use crate::entity::ai::goal::climb_on_top_of_powder_snow::ClimbOnTopOfPowderSnowGoal;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    ConsumableImpl, ConsumeAnimation, EquipmentSlot, UseRemainderImpl,
};
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::ai::behavior::fleeing::{self, Flee};
use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::control::move_control::MoveControl;
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::active_target::ActiveTargetGoal;
use crate::entity::ai::goal::avoid_entity::not_creative_or_spectator;
use crate::entity::ai::goal::leap_at_target::LeapAtTargetGoal;
use crate::entity::ai::goal::stroll_through_village::StrollThroughVillageGoal;
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::item::ItemEntity;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::passive::tamable::is_untamed;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_at_entity::LookAtEntityGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;
use crate::world::game_event::GameEventContext;
use goals::{
    DefendTrustedTargetGoal, EatBerriesGoal, FaceplantGoal, FoxGoal, PerchAndSearchGoal,
    PounceGoal, SearchForItemsGoal, SeekShelterGoal, SleepGoal, StalkPreyGoal, melee_attack,
};

pub const FOX_FOOD: &[&Item] = &[&Item::SWEET_BERRIES, &Item::GLOW_BERRIES];

const FOX_FLEES: &[Flee] = &[
    Flee::new(4, TargetMatch::Type(&EntityType::PLAYER), 16.0, 1.6, 1.4)
        .when(is_not_defending)
        .matching(is_untrusted_player),
    Flee::new(4, TargetMatch::Type(&EntityType::WOLF), 8.0, 1.6, 1.4)
        .when(is_not_defending)
        .matching(is_untamed_wolf),
    Flee::new(4, TargetMatch::Type(&EntityType::POLAR_BEAR), 8.0, 1.6, 1.4).when(is_not_defending),
];

/// Vanilla `landTargetGoal`, `turtleEggTargetGoal` and `fishTargetGoal`.
const fn land_prey(priority: u8) -> Hunt {
    Hunt::new(
        priority,
        TargetMatch::AnyOf(&[&EntityType::CHICKEN, &EntityType::RABBIT]),
        false,
    )
}

const fn fish_prey(priority: u8) -> Hunt {
    Hunt::new(priority, TargetMatch::FISH, false).every(20)
}

/// Red foxes hunt on land first, snow foxes fish first.
const RED_FOX_HUNTS: &[Hunt] = &[land_prey(4), hunting::baby_turtles(4, false), fish_prey(6)];
const SNOW_FOX_HUNTS: &[Hunt] = &[fish_prey(4), land_prey(6), hunting::baby_turtles(6, false)];

/// Vanilla `MAX_CROUCH_AMOUNT`: a full crouch, ready to pounce.
const MAX_CROUCH_AMOUNT: f32 = 5.0;

/// Vanilla `MIN_TICKS_BEFORE_EAT`: how long food stays in the mouth before it is eaten.
const MIN_TICKS_BEFORE_EAT: i32 = 600;
/// From here on the fox chews audibly on the food in its mouth.
const TICKS_BEFORE_CHEWING: i32 = 560;

fn as_fox(mob: &dyn Mob) -> Option<&FoxEntity> {
    mob.cast_any().downcast_ref::<FoxEntity>()
}

fn is_not_defending(mob: &dyn Mob) -> bool {
    as_fox(mob).is_some_and(|fox| !fox.is_defending())
}

/// Sneaking players and the ones the fox trusts are left alone.
fn is_untrusted_player(mob: &dyn Mob, candidate: &dyn EntityBase) -> bool {
    let entity = candidate.get_entity();
    not_creative_or_spectator(mob, candidate)
        && !entity.is_sneaking()
        && as_fox(mob).is_some_and(|fox| !fox.trusts(entity.entity_uuid))
}

fn is_untamed_wolf(_mob: &dyn Mob, candidate: &dyn EntityBase) -> bool {
    candidate.get_mob().is_some_and(is_untamed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum FoxVariant {
    #[default]
    Red = 0,
    Snow = 1,
}

impl FoxVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Snow,
            _ => Self::Red,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Snow => "snow",
        }
    }

    #[must_use]
    pub fn from_name(s: &str) -> Self {
        match s {
            "snow" | "minecraft:snow" => Self::Snow,
            _ => Self::Red,
        }
    }
}

/// Represents a Fox, a passive/neutral mob.
///
/// Wiki: <https://minecraft.wiki/w/Fox>
pub struct FoxEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub flags: AtomicU8,
    /// Vanilla's two trust slots.
    pub trusted: AtomicCell<[Option<Uuid>; 2]>,
    /// Server-side crouch progress, the pounce waits for a full crouch.
    crouch_amount: AtomicCell<f32>,
    ticks_since_eaten: AtomicI32,
}

/// Vanilla `FoxMoveControl`: a sleeping, sitting or faceplanted fox doesn't steer.
#[derive(Default)]
struct FoxMoveControl(MoveControl);

impl Control for FoxMoveControl {}

impl MoveControlTrait for FoxMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if as_fox(mob).is_some_and(FoxEntity::can_move) {
            MoveControlTrait::tick(&mut self.0, mob);
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        MoveControlTrait::set_wanted_position(&mut self.0, x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        MoveControlTrait::strafe(&mut self.0, forward, right);
    }

    fn has_wanted(&self) -> bool {
        MoveControlTrait::has_wanted(&self.0)
    }

    fn get_speed_modifier(&self) -> f64 {
        MoveControlTrait::get_speed_modifier(&self.0)
    }

    fn get_wanted_position(&self) -> Vector3<f64> {
        MoveControlTrait::get_wanted_position(&self.0)
    }
}

impl FoxEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Fox.defineSynchedData`.
        entity.synched_data.define(
            pumpkin_data::tracked_data::fox::DATA_TRUSTED_ID_0,
            None::<uuid::Uuid>,
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::fox::DATA_TRUSTED_ID_1,
            None::<uuid::Uuid>,
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::fox::DATA_TYPE_ID,
            VarInt(pumpkin_data::entity_variant::FoxVariant::DEFAULT.id()),
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::fox::DATA_FLAGS_ID, 0i8);
        let mob_entity = MobEntity::new(entity);
        let fox = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(FoxVariant::Red.id()),
            flags: AtomicU8::new(0),
            trusted: AtomicCell::new([None; 2]),
            crouch_amount: AtomicCell::new(0.0),
            ticks_since_eaten: AtomicI32::new(0),
        };
        fox.mob_entity.set_can_pick_up_loot(true);
        *fox.mob_entity.steered().move_control() = Box::new(FoxMoveControl::default());
        let mob_arc = Arc::new(fox);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let not_defending: fn(&Self) -> bool = |fox| !fox.is_defending();

            goal_selector.add_goal(
                0,
                Box::new(FoxGoal::new(SwimGoal::default()).on_start(Self::clear_states)),
            );
            goal_selector.add_goal(0, Box::new(ClimbOnTopOfPowderSnowGoal));
            goal_selector.add_goal(1, Box::new(FaceplantGoal::default()));
            goal_selector.add_goal(
                2,
                Box::new(FoxGoal::new(*EscapeDangerGoal::new(2.2)).start_if(not_defending)),
            );
            goal_selector.add_goal(
                3,
                Box::new(FoxGoal::new(*BreedGoal::new(1.0)).on_start(Self::clear_states)),
            );
            fleeing::apply(goal_selector, FOX_FLEES);
            goal_selector.add_goal(5, Box::new(StalkPreyGoal));
            goal_selector.add_goal(6, Box::new(PounceGoal));
            goal_selector.add_goal(6, Box::new(SeekShelterGoal::new(1.25)));
            goal_selector.add_goal(7, Box::new(melee_attack(1.2)));
            goal_selector.add_goal(7, Box::new(SleepGoal::new()));
            goal_selector.add_goal(
                8,
                Box::new(
                    FoxGoal::new(FollowParentGoal::new(1.25))
                        .gated(not_defending)
                        .on_start(Self::clear_states),
                ),
            );
            goal_selector.add_goal(
                9,
                Box::new(
                    FoxGoal::new(StrollThroughVillageGoal::new(200))
                        .gated(|fox| {
                            !fox.is_sleeping()
                                && !fox.is_sitting()
                                && !fox.is_defending()
                                && fox.mob_entity.get_target().is_none()
                        })
                        .on_start(Self::clear_states),
                ),
            );
            goal_selector.add_goal(10, Box::new(EatBerriesGoal::new(1.2)));
            goal_selector.add_goal(10, Box::new(LeapAtTargetGoal::new(0.4)));
            goal_selector.add_goal(11, Box::new(WanderAroundGoal::water_avoiding(1.0)));
            goal_selector.add_goal(11, Box::new(SearchForItemsGoal));
            goal_selector.add_goal(
                12,
                Box::new(
                    FoxGoal::new(*LookAtEntityGoal::with_default(
                        mob_weak,
                        &EntityType::PLAYER,
                        24.0,
                    ))
                    .gated(|fox| !fox.is_faceplanted() && !fox.is_interested()),
                ),
            );
            goal_selector.add_goal(13, Box::new(PerchAndSearchGoal::default()));

            mob_arc
                .mob_entity
                .target_selector
                .add_goal(3, Box::new(DefendTrustedTargetGoal::new()));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> FoxVariant {
        FoxVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: FoxVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::fox::TYPE_ID,
            VarInt(variant.id()),
        );
    }

    fn get_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    fn set_flag(&self, flag: u8, value: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if value {
            current | flag
        } else {
            current & !flag
        };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::fox::FLAGS_ID, new_flags);
    }

    #[must_use]
    pub fn is_sitting(&self) -> bool {
        self.get_flag(1)
    }

    pub fn set_sitting(&self, val: bool) {
        self.set_flag(1, val);
    }

    #[must_use]
    pub fn is_crouching(&self) -> bool {
        self.get_flag(4)
    }

    pub fn set_crouching(&self, val: bool) {
        self.set_flag(4, val);
    }

    #[must_use]
    pub fn is_interested(&self) -> bool {
        self.get_flag(8)
    }

    pub fn set_interested(&self, val: bool) {
        self.set_flag(8, val);
    }

    #[must_use]
    pub fn is_pouncing(&self) -> bool {
        self.get_flag(16)
    }

    pub fn set_pouncing(&self, val: bool) {
        self.set_flag(16, val);
    }

    #[must_use]
    pub fn is_sleeping(&self) -> bool {
        self.get_flag(32)
    }

    pub fn set_sleeping(&self, val: bool) {
        self.set_flag(32, val);
    }

    #[must_use]
    pub fn is_faceplanted(&self) -> bool {
        self.get_flag(64)
    }

    pub fn set_faceplanted(&self, val: bool) {
        self.set_flag(64, val);
    }

    #[must_use]
    pub fn is_defending(&self) -> bool {
        self.get_flag(128)
    }

    pub fn set_defending(&self, val: bool) {
        self.set_flag(128, val);
    }

    /// With both slots taken the second is replaced.
    pub fn add_trusted(&self, uuid: Uuid) {
        let trusted = match self.trusted.load() {
            [Some(first), _] => [Some(first), Some(uuid)],
            [None, _] => [Some(uuid), None],
        };
        self.trusted.store(trusted);
    }

    #[must_use]
    pub fn trusts(&self, uuid: Uuid) -> bool {
        self.trusted.load().contains(&Some(uuid))
    }

    #[must_use]
    pub fn is_fully_crouched(&self) -> bool {
        self.crouch_amount.load() == MAX_CROUCH_AMOUNT
    }

    /// Vanilla `setTargetGoals`, run on spawn and load: the hunts in the variant's order.
    fn set_target_goals(&self) {
        let targets = &self.mob_entity.target_selector;
        targets.remove_goal::<ActiveTargetGoal>(self);
        let hunts = match self.get_variant() {
            FoxVariant::Red => RED_FOX_HUNTS,
            FoxVariant::Snow => SNOW_FOX_HUNTS,
        };
        hunting::apply(targets, &self.mob_entity, hunts);
    }

    /// Vanilla `Fox.tick` after the AI step.
    fn tick_states(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if !self.mob_entity.is_no_ai() {
            let in_water = entity.touching_water.load(Ordering::Relaxed);
            if self.is_sleeping()
                && (in_water || self.mob_entity.get_target().is_some() || is_thundering(&world))
            {
                self.set_sleeping(false);
            }
            if self.is_sitting() && (in_water || self.is_sleeping()) {
                self.set_sitting(false);
            }
            if self.is_faceplanted() && game_random().random::<f32>() < 0.2 {
                let pos = entity.block_pos.load();
                let state = world.get_block_state_id(&pos);
                world.sync_world_event(
                    WorldEvent::ParticlesAndSoundDestroyBlock,
                    pos,
                    i32::from(state.as_u16()),
                );
            }
        }
        let crouch = if self.is_crouching() {
            (self.crouch_amount.load() + 0.2).min(MAX_CROUCH_AMOUNT)
        } else {
            0.0
        };
        self.crouch_amount.store(crouch);
    }

    pub fn clear_states(&self) {
        self.set_interested(false);
        self.set_crouching(false);
        self.set_sitting(false);
        self.set_sleeping(false);
        self.set_defending(false);
        self.set_faceplanted(false);
    }

    #[must_use]
    pub fn can_move(&self) -> bool {
        !self.is_sleeping() && !self.is_sitting() && !self.is_faceplanted()
    }

    /// The item in the fox's mouth, its main hand.
    #[must_use]
    pub fn mouth_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    fn set_mouth_item(&self, stack: ItemStack) {
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, stack);
    }

    fn can_eat(&self, mouth_item: &ItemStack) -> bool {
        is_consumable_food(mouth_item)
            && self.mob_entity.get_target().is_none()
            && self.get_entity().on_ground.load(Ordering::Relaxed)
            && !self.is_sleeping()
    }

    /// The eating part of vanilla `Fox.aiStep`.
    fn tick_eating(&self) {
        let ticks = self.ticks_since_eaten.fetch_add(1, Ordering::Relaxed) + 1;
        let mouth_item = self.mouth_item();
        if !self.can_eat(&mouth_item) {
            return;
        }
        if ticks > MIN_TICKS_BEFORE_EAT {
            let remaining = self.finish_using_item(mouth_item);
            self.set_mouth_item(remaining);
            self.ticks_since_eaten.store(0, Ordering::Relaxed);
        } else if ticks > TICKS_BEFORE_CHEWING && self.get_random().random::<f32>() < 0.1 {
            self.play_eating_sound();
            let entity = self.get_entity();
            entity.world.load().send_entity_status(
                entity,
                EntityStatus::FoxEat,
                ActorEventID::None,
            );
        }
    }

    /// Vanilla `ItemStack.finishUsingItem` for the food in the mouth. A fox has nowhere to put
    /// extra remainders, so like vanilla they are lost.
    fn finish_using_item(&self, mut stack: ItemStack) -> ItemStack {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let world = entity.world.load();
        living.play_consume_sound(self, &stack);
        living.apply_suspicious_stew_effects(&stack);
        if let Some(consumable) = stack.get_data_component::<ConsumableImpl>() {
            // Vanilla `FoodProperties.onConsume` plays the consume sound a second time.
            let rng = self.get_random();
            let pitch = 0.4f32.mul_add(rng.random::<f32>() - rng.random::<f32>(), 1.0);
            world.play_sound_event_fine(
                &consumable.sound_event,
                SoundCategory::Neutral,
                &entity.pos.load(),
                1.0,
                pitch,
            );
            living.apply_consumable_effects(self, &stack);
            let event = if consumable.animation == ConsumeAnimation::Drink {
                GameEvent::Drink
            } else {
                GameEvent::Eat
            };
            world.emit_game_event_with_context(
                event.name(),
                entity.pos.load(),
                &GameEventContext {
                    source_entity: Some(self),
                },
            );
        }
        let count_before = stack.item_count;
        stack.decrement(1);
        match stack.get_data_component::<UseRemainderImpl>().cloned() {
            Some(remainder) => remainder.convert_into_remainder(stack, count_before, false, |_| {}),
            None => stack,
        }
    }

    /// Vanilla `spitOutItem`.
    fn spit_out_item(&self, stack: ItemStack) {
        if stack.is_empty() {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        let look = entity.rotation();
        let item = ItemEntity::new(
            Entity::new(
                world.clone(),
                Vector3::new(
                    pos.x + f64::from(look.x),
                    pos.y + 1.0,
                    pos.z + f64::from(look.z),
                ),
                &EntityType::ITEM,
            ),
            stack,
        );
        item.set_pickup_delay(40);
        item.set_thrower(entity.entity_uuid);
        self.play_entity_sound(Sound::EntityFoxSpit, 1.0, 1.0);
        world.spawn_entity(Arc::new(item));
    }

    /// Vanilla `dropItemStack`: the rest of a picked up stack, with no pickup delay.
    fn drop_item_stack(&self, stack: ItemStack) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let item = ItemEntity::new(
            Entity::new(world.clone(), entity.pos.load(), &EntityType::ITEM),
            stack,
        );
        item.set_pickup_delay(0);
        world.spawn_entity(Arc::new(item));
    }

    /// Vanilla `populateDefaultEquipmentSlots`: one fox in five spawns with an item in its mouth.
    fn populate_default_equipment(&self) {
        let rng = game_random();
        if rng.random::<f32>() >= 0.2 {
            return;
        }
        let odds = rng.random::<f32>();
        let item = if odds < 0.05 {
            &Item::EMERALD
        } else if odds < 0.2 {
            &Item::EGG
        } else if odds < 0.4 {
            if rng.random::<bool>() {
                &Item::RABBIT_FOOT
            } else {
                &Item::RABBIT_HIDE
            }
        } else if odds < 0.6 {
            &Item::WHEAT
        } else if odds < 0.8 {
            &Item::LEATHER
        } else {
            &Item::FEATHER
        };
        self.set_mouth_item(ItemStack::new(1, item));
    }
}

/// Vanilla `Fox.isConsumableFood`.
fn is_consumable_food(stack: &ItemStack) -> bool {
    stack.has_data_component(DataComponent::Food)
        && stack.has_data_component(DataComponent::Consumable)
}

impl AgeableMob for FoxEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for FoxEntity {
    fn play_eating_sound(&self) {
        self.play_entity_sound(Sound::EntityFoxEat, 1.0, 1.0);
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FOX_FOOD)
            || FOX_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }

    /// Vanilla `Fox.getBreedOffspring`: the cub takes either parent's variant.
    /// No `finalizeSpawn`, so like vanilla its hunts wait for the next load.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::FOX,
        ));
        let partner_variant = partner
            .cast_any()
            .downcast_ref::<Self>()
            .map_or_else(|| self.get_variant(), Self::get_variant);
        baby.set_variant(if self.get_random().random::<bool>() {
            self.get_variant()
        } else {
            partner_variant
        });
        baby
    }
}

/// Vanilla `Level.isThundering`.
fn is_thundering(world: &World) -> bool {
    let weather = world
        .weather
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    weather.thunder_level * weather.rain_level > 0.9
}

/// Vanilla `Fox.playAmbientSound` screech volume.
const FOX_SCREECH_VOLUME: f32 = 2.0;

impl Mob for FoxEntity {
    fn get_ambient_sound(&self) -> Option<Sound> {
        if self.is_sleeping() {
            return Some(Sound::EntityFoxSleep);
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        if !world.is_bright_outside() && self.get_random().random::<f32>() < 0.1 {
            let search = entity.bounding_box.load().expand(16.0, 16.0, 16.0);
            if world
                .get_players_at_box(&search)
                .iter()
                .all(|player| player.is_spectator())
            {
                return Some(Sound::EntityFoxScreech);
            }
        }
        Some(Sound::EntityFoxAmbient)
    }

    fn play_ambient_sound(&self) {
        match self.get_ambient_sound() {
            Some(Sound::EntityFoxScreech) => self.mob_entity.living_entity.make_sound(
                Sound::EntityFoxScreech,
                self.sound_source(),
                FOX_SCREECH_VOLUME,
                self.mob_entity.living_entity.get_pitch(),
            ),
            Some(sound) => self.make_sound(sound),
            None => {}
        }
    }

    fn can_dispenser_equip_into_slot(
        &self,
        slot: &pumpkin_data::data_component_impl::EquipmentSlot,
    ) -> bool {
        *slot == pumpkin_data::data_component_impl::EquipmentSlot::MAIN_HAND
            && self.mob_entity.can_pick_up_loot()
    }

    /// Vanilla `Fox.canHoldItem`: anything in an empty mouth, food over a held non-food item.
    fn can_hold_item(&self, stack: &ItemStack) -> bool {
        let held = self.mouth_item();
        held.is_empty()
            || self.ticks_since_eaten.load(Ordering::Relaxed) > 0
                && is_consumable_food(stack)
                && !is_consumable_food(&held)
    }

    /// Vanilla `Fox.pickUpItem`: one item into the mouth, the rest dropped, the old one spat out.
    fn pick_up_item(&self, item: &ItemEntity) {
        let mut stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !self.can_hold_item(&stack) {
            return;
        }
        let living = &self.mob_entity.living_entity;
        if !living.pickup(item.get_entity(), 1) {
            return;
        }
        let count = stack.item_count;
        if count > 1 {
            self.drop_item_stack(stack.split(count - 1));
        }
        self.spit_out_item(self.mouth_item());
        living.on_item_pickup(item);
        self.set_mouth_item(stack.split(1));
        self.mob_entity
            .set_guaranteed_drop(&EquipmentSlot::MAIN_HAND);
        item.get_entity().remove();
        self.ticks_since_eaten.store(0, Ordering::Relaxed);
    }

    /// Vanilla `Fox.dropAllDeathLoot`: whatever is in the mouth always drops.
    fn drop_before_death_loot(&self) {
        let stack = self.mouth_item();
        if !stack.is_empty() {
            self.get_entity().spawn_at_location(stack);
            self.set_mouth_item(ItemStack::EMPTY.clone());
        }
    }

    fn on_offspring_spawned_from_egg(&self, spawner: &Player, offspring: &dyn EntityBase) {
        if let Some(fox) = offspring.cast_any().downcast_ref::<Self>() {
            fox.add_trusted(spawner.gameprofile.id);
        }
    }

    /// Vanilla `FoxLookControl`: a sleeping fox keeps its head still.
    fn ticks_look_control(&self) -> bool {
        !self.is_sleeping()
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let (variant, size) = if let Some(SpawnGroupData::Fox { variant, size }) = group_data {
            (variant, size)
        } else {
            let biome = world.get_biome(&self.get_entity().block_pos.load());
            let bare = biome
                .registry_id
                .strip_prefix("minecraft:")
                .unwrap_or(biome.registry_id);
            let snowy = tag::WorldgenBiome::MINECRAFT_SPAWNS_SNOW_FOXES
                .0
                .contains(&bare);
            (
                if snowy {
                    FoxVariant::Snow
                } else {
                    FoxVariant::Red
                },
                0,
            )
        };
        self.set_variant(variant);
        if size >= 2 {
            self.get_entity().set_age(-24000);
        }
        self.set_target_goals();
        self.populate_default_equipment();
        self.mob_entity.finalize_spawn_base();
        Some(SpawnGroupData::Fox {
            variant,
            size: size + 1,
        })
    }

    /// Vanilla `Fox.aiStep` before `super.aiStep`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let living = &self.mob_entity.living_entity;
        if !self.mob_entity.is_no_ai() && living.is_alive() {
            self.tick_eating();
            // A lost target ends the stalk.
            if (self.is_crouching() || self.is_interested())
                && self
                    .mob_entity
                    .get_target()
                    .is_none_or(|target| !target.get_entity().is_alive())
            {
                self.set_crouching(false);
                self.set_interested(false);
            }
        }
        if self.is_sleeping() || living.is_dead_or_dying() {
            living.jumping.store(false, Ordering::Relaxed);
            let input = living.movement_input.load();
            living.movement_input.store(Vector3::new(0.0, input.y, 0.0));
        }
    }

    fn post_tick(&self) {
        self.tick_states();
        if self.is_defending() && game_random().random::<f32>() < 0.05 {
            self.get_entity().play_sound(Sound::EntityFoxAggro);
        }
    }

    fn apply_mob_target(&self, target: Option<Arc<dyn EntityBase>>) {
        if target.is_none() && self.is_defending() {
            self.set_defending(false);
        }
        self.mob_entity.set_target(target);
    }

    fn look_resets_pitch(&self) -> bool {
        !self.is_pouncing()
            && !self.is_crouching()
            && !self.is_interested()
            && !self.is_faceplanted()
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string("Type", self.get_variant().as_str().to_string());
        nbt.put_bool("Sleeping", self.is_sleeping());
        nbt.put_bool("Sitting", self.is_sitting());
        nbt.put_bool("Crouching", self.is_crouching());
        let trusted = self.trusted.load().into_iter().flatten().map(NbtTag::uuid);
        nbt.put_list("Trusted", trusted.collect());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant_str) = nbt.get_string("Type") {
            self.set_variant(FoxVariant::from_name(variant_str));
        }
        if let Some(sleeping) = nbt.get_bool("Sleeping") {
            self.set_sleeping(sleeping);
        }
        if let Some(sitting) = nbt.get_bool("Sitting") {
            self.set_sitting(sitting);
        }
        if let Some(crouching) = nbt.get_bool("Crouching") {
            self.set_crouching(crouching);
        }
        self.trusted.store([None; 2]);
        for uuid in nbt
            .get_list("Trusted")
            .unwrap_or_default()
            .iter()
            .filter_map(NbtTag::extract_uuid)
        {
            self.add_trusted(uuid);
        }
        self.set_target_goals();
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_set_variant_name(&self, name: &str) {
        self.set_variant(FoxVariant::from_name(name));
    }

    fn on_bred_by(&self, player: Uuid) {
        self.add_trusted(player);
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::fox::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::fox::TYPE_ID,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::fox::FLAGS_ID,
            self.flags.load(Ordering::Relaxed),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack)
    }
}
