use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use pumpkin_data::block_properties::WhiteBedLikeProperties;
use pumpkin_data::environment_attribute::EnvironmentAttribute;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use pumpkin_data::cat_sound_variant::CatSoundVariant;
use pumpkin_data::cat_variant::CatVariant;
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use uuid::Uuid;

use crate::entity::custom_sound::CustomSound;
use crate::entity::passive::sheep::get_mixed_color;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::behavior::{
        feline,
        fleeing::{self, Flee},
        hunting::{self, Hunt},
    },
    ai::goal::{
        Controls, Goal, GoalReach, active_target::TargetCondition,
        cat_lie_on_block::CatLieOnBlockGoal, cat_sit_on_block::CatSitOnBlockGoal,
        escape_danger::EscapeDangerGoal, follow_owner::FollowOwnerGoal,
        sit_when_ordered_to::SitWhenOrderedToGoal, swim::SwimGoal, tempt::TemptGoal,
    },
    ai::pathfinder::NavigatorGoal,
    ai::target_match::TargetMatch,
    mob::{Mob, MobEntity, spawn::SpawnGroupData},
    passive::{
        animal::Animal,
        tamable::{TamableAnimal, TamableData, is_untamed},
    },
    player::Player,
};
use crate::world::World;

const CAT_FLEES: &[Flee] = &[feline::flee_players(4, is_untamed)];

const CAT_HUNTS: &[Hunt] = &[
    Hunt::new(1, TargetMatch::Type(&EntityType::RABBIT), false).when(TargetCondition::Untamed),
    hunting::baby_turtles(1, false).when(TargetCondition::Untamed),
];

pub struct CatEntity {
    pub mob_entity: MobEntity,
    pub variant: AtomicU8,
    pub sound_variant: AtomicU8,
    pub collar_color: AtomicU8,
    pub tamable_data: TamableData,
    pub ageable_data: AgeableData,
    pub is_lying: AtomicBool,
    pub relax_state_one: AtomicBool,
}

impl CatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Cat.defineSynchedData`.
        crate::entity::passive::tamable::define_tamable_animal_synched_data(&entity);
        entity.synched_data.define(
            pumpkin_data::tracked_data::cat::DATA_VARIANT_ID,
            VarInt(i32::from(pumpkin_data::cat_variant::CatVariant::Black.id())),
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::cat::DATA_SOUND_VARIANT_ID,
            VarInt(i32::from(
                pumpkin_data::cat_sound_variant::CatSoundVariant::Classic.id(),
            )),
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::cat::IS_LYING, false);
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::cat::RELAX_STATE_ONE, false);
        entity.synched_data.define(
            pumpkin_data::tracked_data::cat::DATA_COLLAR_COLOR,
            VarInt(pumpkin_data::dye_color::DyeColor::Red as i32),
        );
        let mob_entity = MobEntity::new(entity);
        let cat = Self {
            mob_entity,
            variant: AtomicU8::new(CatVariant::Black.id()),
            sound_variant: AtomicU8::new(0), // Default to classic
            collar_color: AtomicU8::new(14), // Default to red
            tamable_data: TamableData::default(),
            ageable_data: AgeableData::default(),
            is_lying: AtomicBool::new(false),
            relax_state_one: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(cat);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Goal 1: SwimGoal (FloatGoal)
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            // Goal 1: TamableAnimalPanicGoal (EscapeDangerGoal)
            goal_selector.add_goal(
                1,
                EscapeDangerGoal::tamable(1.5, &tag::DamageType::MINECRAFT_PANIC_CAUSES),
            );
            // Goal 2: SitWhenOrderedToGoal
            goal_selector.add_goal(2, Box::new(SitWhenOrderedToGoal::new()));
            goal_selector.add_goal(3, Box::new(CatRelaxOnOwnerGoal::default()));
            // Goal 4: CatTemptGoal
            goal_selector.add_goal(4, Box::new(CatTemptGoal::new()));
            // Goal 4: CatAvoidEntityGoal (when untamed)
            fleeing::apply(goal_selector, CAT_FLEES);
            goal_selector.add_goal(5, CatLieOnBlockGoal::new(1.1, 8));
            // Goal 6: FollowOwnerGoal
            goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 5.0));
            goal_selector.add_goal(7, CatSitOnBlockGoal::new(0.8));
            // Goal 8-12: LeapAtTarget, OcelotAttack, Breed, Stroll, LookAtPlayer
            feline::apply_tail(goal_selector, mob_weak, 8);
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            // NonTameRandomTargetGoal for Rabbit and Turtle
            hunting::apply(target_selector, &mob_arc.mob_entity, CAT_HUNTS);
        };

        mob_arc
    }

    pub fn get_tame_flags(&self) -> u8 {
        let mut flags = 0u8;
        if self.is_in_sitting_pose() {
            flags |= 0x01;
        }
        if self.is_tame() {
            flags |= 0x04;
        }
        flags
    }

    pub fn is_lying(&self) -> bool {
        self.is_lying.load(Ordering::Relaxed)
    }

    pub fn is_relax_state_one(&self) -> bool {
        self.relax_state_one.load(Ordering::Relaxed)
    }

    pub fn get_collar_color(&self) -> u8 {
        self.collar_color.load(Ordering::Relaxed)
    }

    pub fn set_collar_color(&self, color: u8) {
        self.collar_color.store(color, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::CAT_COLLAR_COLOR,
            VarInt(color as i32),
        );
    }

    pub fn set_tame(&self, tame: bool, owner: Option<Uuid>) {
        self.tamable_data.is_tame.store(tame, Ordering::Relaxed);
        self.set_owner(owner);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::TAMEABLE_FLAGS,
            self.get_tame_flags(),
        );
        entity.set_synced_data(pumpkin_data::tracked_data::cat::OWNER_UUID, owner);
    }

    pub fn set_variant(&self, variant: u8) {
        self.variant.store(variant, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::CAT_VARIANT,
            VarInt(variant as i32),
        );
    }

    pub fn set_lying(&self, lying: bool) {
        self.is_lying.store(lying, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::cat::IS_LYING, lying);
    }

    pub fn set_relax_state_one(&self, relax: bool) {
        self.relax_state_one.store(relax, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::cat::RELAX_STATE_ONE, relax);
    }

    pub fn set_sound_variant(&self, sound_variant: CatSoundVariant) {
        self.sound_variant
            .store(sound_variant as u8, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::SOUND_VARIANT,
            VarInt(sound_variant as u8 as i32),
        );
    }
}

#[must_use]
pub fn as_cat(mob: &dyn Mob) -> Option<&CatEntity> {
    mob.cast_any().downcast_ref::<CatEntity>()
}

/// Vanilla `ChestBlock.isCatSittingOnChest`.
#[must_use]
pub fn is_cat_sitting_on(world: &World, pos: &BlockPos) -> bool {
    let above = BoundingBox::from_block(&pos.up());
    world.get_entities_at_box(&above).iter().any(|entity| {
        entity
            .get_mob()
            .and_then(as_cat)
            .is_some_and(TamableAnimal::is_in_sitting_pose)
    })
}

impl CatEntity {
    fn sound_variant(&self) -> CatSoundVariant {
        CatSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed)).unwrap_or_default()
    }
}

/// Vanilla `Cat.CatTemptGoal`: only untamed cats follow fish, and a cat sometimes picks the
/// player it follows and stops being scared of them.
struct CatTemptGoal {
    tempt: TemptGoal,
    selected_player: Option<uuid::Uuid>,
}

impl CatTemptGoal {
    fn new() -> Self {
        Self {
            tempt: TemptGoal::with_tag(0.6, &tag::Item::MINECRAFT_CAT_FOOD, true),
            selected_player: None,
        }
    }

    fn is_selected(&self) -> bool {
        self.selected_player.is_some()
            && self.selected_player
                == self
                    .tempt
                    .target_player()
                    .map(|player| player.gameprofile.id)
    }
}

impl Goal for CatTemptGoal {
    fn tick_reach(&self, mob: &dyn Mob, running: bool) -> Option<GoalReach> {
        self.tempt.tick_reach(mob, running)
    }

    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.tempt.can_start(mob) && as_cat(mob).is_some_and(|cat| !cat.is_tame())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.tempt.set_can_scare(!self.is_selected());
        self.tempt.should_continue(mob) && as_cat(mob).is_some_and(|cat| !cat.is_tame())
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.tempt.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.tempt.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.tempt.tick(mob);
        if self.selected_player.is_none()
            && mob.get_random().random_range(0..self.get_tick_count(600)) == 0
        {
            self.selected_player = self
                .tempt
                .target_player()
                .map(|player| player.gameprofile.id);
        } else if mob.get_random().random_range(0..self.get_tick_count(500)) == 0 {
            self.selected_player = None;
        }
    }

    fn should_run_every_tick(&self) -> bool {
        self.tempt.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.tempt.controls()
    }
}

/// Vanilla `CatRelaxOnOwnerGoal.onBedTicks` threshold before the cat lies down.
const RELAX_ON_BED_TICKS: i32 = 16;

/// Vanilla `Cat.CatRelaxOnOwnerGoal`: a tame cat lies down at the foot of its sleeping owner's
/// bed, and may leave a morning gift when the owner wakes up.
#[derive(Default)]
struct CatRelaxOnOwnerGoal {
    owner_player: Option<Arc<Player>>,
    goal_pos: Option<BlockPos>,
    on_bed_ticks: i32,
}

impl CatRelaxOnOwnerGoal {
    fn space_is_occupied(cat: &CatEntity, goal_pos: BlockPos) -> bool {
        let world = cat.get_entity().world.load();
        let area = BoundingBox::from_block(&goal_pos).expand(2.0, 2.0, 2.0);
        world.get_entities_at_box(&area).iter().any(|other| {
            other.get_entity().entity_id != cat.get_entity().entity_id
                && other
                    .get_mob()
                    .and_then(as_cat)
                    .is_some_and(|other| other.is_lying() || other.is_relax_state_one())
        })
    }

    fn move_to_goal(cat: &CatEntity, goal_pos: BlockPos) {
        let steered = cat.mob_entity.steered();
        steered.navigator().set_progress(NavigatorGoal::new(
            cat.get_entity().pos.load(),
            goal_pos.0.to_f64(),
            1.1,
        ));
    }

    /// Vanilla `CatRelaxOnOwnerGoal.giveMorningGift`.
    fn give_morning_gift(cat: &CatEntity) {
        let rng = cat.get_random();
        let entity = cat.get_entity();
        let origin = cat.get_entity().get_leash_holder().map_or_else(
            || entity.block_pos.load(),
            |holder| holder.get_entity().block_pos.load(),
        );
        let x = f64::from(origin.0.x + rng.random_range(0..11) - 5);
        let y = f64::from(origin.0.y + rng.random_range(0..5) - 2);
        let z = f64::from(origin.0.z + rng.random_range(0..11) - 5);
        let living = &cat.mob_entity.living_entity;
        if let Some(target) = living.random_teleport_target(
            x,
            y,
            z,
            |block| block.has_tag(&tag::Block::MINECRAFT_CAT_DOES_NOT_TELEPORT_TO),
            |_, _| true,
        ) {
            let world = entity.world.load_full();
            entity.teleport(target, None, None, &world);
            cat.mob_entity.steered().navigator().stop();
        }
        let cat_pos = entity.block_pos.load();
        let body_yaw = entity.body_yaw.load().to_radians();
        let world = entity.world.load_full();
        let drop_pos = Vector3::new(
            f64::from(cat_pos.0.x) - f64::from(body_yaw.sin()),
            f64::from(cat_pos.0.y),
            f64::from(cat_pos.0.z) + f64::from(body_yaw.cos()),
        );
        crate::entity::mob::drop_from_gift_loot_table_with(
            cat,
            CAT_MORNING_GIFT_LOOT_TABLE,
            |stack| {
                let item = crate::entity::item::ItemEntity::new(
                    Entity::new(world.clone(), drop_pos, &EntityType::ITEM),
                    stack,
                );
                world.spawn_entity(Arc::new(item));
            },
        );
    }
}

const CAT_MORNING_GIFT_LOOT_TABLE: &str = "gameplay/cat_morning_gift";

impl Goal for CatRelaxOnOwnerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(cat) = as_cat(mob) else {
            return false;
        };
        if !cat.is_tame() || cat.is_ordered_to_sit() {
            return false;
        }
        let entity = cat.get_entity();
        let world = entity.world.load();
        let Some(owner) = cat
            .get_owner()
            .and_then(|uuid| world.get_player_by_uuid(uuid))
        else {
            return false;
        };
        self.owner_player = Some(owner.clone());
        if !owner.is_sleeping()
            || entity
                .pos
                .load()
                .squared_distance_to_vec(&owner.get_entity().pos.load())
                > 100.0
        {
            return false;
        }
        let owner_pos = owner.get_entity().block_pos.load();
        let (block, state_id) = world.get_block_and_state_id(&owner_pos);
        if !block.has_tag(&tag::Block::MINECRAFT_CATS_CAN_LIE_ON) {
            return false;
        }
        // Every block in `cats_can_lie_on` is a bed, which always has a facing.
        let facing = WhiteBedLikeProperties::from_state_id(state_id).facing;
        let goal_pos = owner_pos.offset(facing.opposite().to_offset());
        self.goal_pos = Some(goal_pos);
        !Self::space_is_occupied(cat, goal_pos)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(cat) = as_cat(mob) else {
            return false;
        };
        cat.is_tame()
            && !cat.is_ordered_to_sit()
            && self
                .owner_player
                .as_ref()
                .is_some_and(|owner| owner.is_sleeping())
            && self
                .goal_pos
                .is_some_and(|goal_pos| !Self::space_is_occupied(cat, goal_pos))
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(cat) = as_cat(mob)
            && let Some(goal_pos) = self.goal_pos
        {
            cat.set_in_sitting_pose(false);
            Self::move_to_goal(cat, goal_pos);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(cat) = as_cat(mob) else {
            return;
        };
        cat.set_lying(false);
        if let Some(owner) = &self.owner_player
            && owner.get_sleep_timer() >= 100
        {
            let entity = cat.get_entity();
            let world = entity.world.load();
            let gift_chance = world.environment_attributes().get_value_f32(
                EnvironmentAttribute::GameplayCatWakingUpGiftChance,
                &entity.block_pos.load(),
            );
            if cat.get_random().random::<f32>() < gift_chance {
                Self::give_morning_gift(cat);
            }
        }
        self.on_bed_ticks = 0;
        cat.set_relax_state_one(false);
        cat.mob_entity.steered().navigator().stop();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(cat) = as_cat(mob) else {
            return;
        };
        let (Some(owner), Some(goal_pos)) = (&self.owner_player, self.goal_pos) else {
            return;
        };
        cat.set_in_sitting_pose(false);
        Self::move_to_goal(cat, goal_pos);
        let distance_sq = cat
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&owner.get_entity().pos.load());
        if distance_sq < 2.5 {
            self.on_bed_ticks += 1;
            if self.on_bed_ticks > self.get_tick_count(RELAX_ON_BED_TICKS) {
                cat.set_lying(true);
                cat.set_relax_state_one(false);
            } else {
                cat.mob_entity.look_at(owner.as_ref(), 45.0, 45.0);
                cat.set_relax_state_one(true);
            }
        } else {
            cat.set_lying(false);
        }
    }
}

impl CustomSound for CatEntity {
    fn death_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        let is_baby = self.get_entity().age.load(Ordering::Relaxed) < 0;
        let sound_variant = CatSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        Some(sound_variant.death_sound(is_baby))
    }

    fn hurt_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        let is_baby = self.get_entity().age.load(Ordering::Relaxed) < 0;
        let sound_variant = CatSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        Some(sound_variant.hurt_sound(is_baby))
    }
}

impl Animal for CatEntity {
    /// Vanilla `Cat.getBreedOffspring`: either parent's variant, and a tame parent's owner with
    /// the collars mixed.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let baby = Self::new(Entity::new(
            world.clone(),
            entity.pos.load(),
            &EntityType::CAT,
        ));
        if let Some(partner) = partner.cast_any().downcast_ref::<Self>() {
            let parent = if self.get_random().random::<bool>() {
                self
            } else {
                partner
            };
            baby.set_variant(parent.variant.load(Ordering::Relaxed));
            if self.is_tame() {
                baby.set_tame(true, self.get_owner());
                let collar = get_mixed_color(
                    &world,
                    DyeColor::from(self.get_collar_color()),
                    DyeColor::from(partner.get_collar_color()),
                );
                baby.set_collar_color(collar.id());
            }
        }
        baby
    }

    fn play_eating_sound(&self) {
        let sound_variant = CatSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        self.play_entity_sound(
            sound_variant.eat_sound(self.mob_entity.living_entity.is_baby()),
            1.0,
            1.0,
        );
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        let item = item_stack.get_item();
        item.has_tag(&tag::Item::MINECRAFT_CAT_FOOD) || item == &Item::COD || item == &Item::SALMON
    }
}

impl AgeableMob for CatEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl TamableAnimal for CatEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

/// Vanilla `DimensionType.MOON_BRIGHTNESS_PER_PHASE`.
const MOON_BRIGHTNESS_PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
/// The `moon_brightness` minimum in `data/minecraft/cat_variant/all_black.json`.
const ALL_BLACK_MIN_MOON_BRIGHTNESS: f32 = 0.9;

impl CatEntity {
    /// Vanilla `VariantUtils.selectVariantToSpawn` over the `cat_variant` spawn conditions: the
    /// all-black cat wins inside `#cats_spawn_as_black` structures and joins the others under a
    /// full moon; every other variant always matches at priority 0.
    fn select_variant_to_spawn(&self, world: &World) -> CatVariant {
        let pos = self.get_entity().block_pos.load();
        if world.is_in_structure_piece(
            &pos,
            &pumpkin_data::tag::WorldgenStructure::MINECRAFT_CATS_SPAWN_AS_BLACK,
        ) {
            return CatVariant::AllBlack;
        }
        let moon_phase = world.environment_attributes().get_value_moon_phase(&pos);
        let all_black_matches = MOON_BRIGHTNESS_PER_PHASE[usize::from(moon_phase.index())]
            >= ALL_BLACK_MIN_MOON_BRIGHTNESS;
        let candidates = CatVariant::all()
            .iter()
            .filter(|variant| all_black_matches || **variant != CatVariant::AllBlack)
            .collect::<Vec<_>>();
        *candidates[self.get_random().random_range(0..candidates.len())]
    }
}

impl Mob for CatEntity {
    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        let is_baby = self.get_entity().age.load(Ordering::Relaxed) < 0;
        let sound_variant = CatSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        Some(if !self.is_tame() {
            sound_variant.stray_ambient_sound()
        } else if self.mob_entity.is_in_love() {
            sound_variant.purr_sound(is_baby)
        } else if self.get_random().random_range(0..4) == 0 {
            sound_variant.purreow_sound(is_baby)
        } else {
            sound_variant.ambient_sound(is_baby)
        })
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_super(group_data);
        self.set_variant(self.select_variant_to_spawn(world).id());
        let sound_variants = CatSoundVariant::ALL;
        self.set_sound_variant(
            sound_variants[self.get_random().random_range(0..sound_variants.len())],
        );
        group_data
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        let mut meta = crate::entity::mob::MobMetadata::new();
        if self.get_entity().age.load(Ordering::Relaxed) < 0 {
            meta = meta.baby(true);
        }
        if self.is_tame() {
            meta = meta.tamed(true);
        }
        if self.is_in_sitting_pose() {
            meta = meta.sitting(true);
        }
        meta = meta.collar_color(
            pumpkin_data::tracked_data::cat::CAT_COLLAR_COLOR,
            pumpkin_data::dye_color::DyeColor::from(self.collar_color.load(Ordering::Relaxed)),
        );
        Some(meta)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let variant_id = u32::from(self.variant.load(Ordering::Relaxed));
        let variant_str =
            self.resolve_dynamic_entry_id("minecraft:cat_variant", variant_id, |id| {
                CatVariant::all()
                    .get(id as usize)
                    .map_or("minecraft:tabby", CatVariant::asset_id)
                    .to_string()
            });
        nbt.put_string("variant", variant_str);
        let sound_variant = CatSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        nbt.put_string(
            "sound_variant",
            format!("minecraft:{}", sound_variant.to_name()),
        );
        nbt.put_byte(
            "CollarColor",
            self.collar_color.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant_str) = nbt.get_string("variant")
            && let Some(id) =
                self.resolve_dynamic_raw_id("minecraft:cat_variant", variant_str, |n| {
                    CatVariant::from_name(n).map(|v| u32::from(v.id()))
                })
            && let Ok(variant_id) = u8::try_from(id)
        {
            self.set_variant(variant_id);
        }
        if let Some(sound_str) = nbt.get_string("sound_variant")
            && let Some(sound_variant) = CatSoundVariant::from_name(sound_str)
        {
            self.sound_variant
                .store(sound_variant as u8, Ordering::Relaxed);
        }
        if let Some(collar) = nbt.get_byte("CollarColor") {
            self.collar_color.store(collar as u8, Ordering::Relaxed);
        } else if let Some(collar_int) = nbt.get_int("CollarColor") {
            self.collar_color.store(collar_int as u8, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(id) = self.resolve_dynamic_raw_id("minecraft:cat_variant", name, |n| {
            CatVariant::from_name(n).map(|v| u32::from(v.id()))
        }) && let Ok(variant_id) = u8::try_from(id)
        {
            self.set_variant(variant_id);
        }
    }

    fn mob_set_sound_variant_name(&self, name: &str) {
        if let Some(v) = CatSoundVariant::from_name(name) {
            self.set_sound_variant(v);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::cat::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::TAMEABLE_FLAGS,
            self.get_tame_flags(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::OWNER_UUID,
            self.get_owner(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::CAT_VARIANT,
            VarInt(self.variant.load(Ordering::Relaxed) as i32),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::IS_LYING,
            self.is_lying.load(Ordering::Relaxed),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::RELAX_STATE_ONE,
            self.relax_state_one.load(Ordering::Relaxed),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::CAT_COLLAR_COLOR,
            VarInt(self.collar_color.load(Ordering::Relaxed) as i32),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::cat::SOUND_VARIANT,
            VarInt(self.sound_variant.load(Ordering::Relaxed) as i32),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();
        let is_food = self.is_food(item_stack);

        if self.is_tame() {
            if self.get_owner_uuid() == Some(player.gameprofile.id) {
                if item.has_tag(&tag::Item::MINECRAFT_CAT_COLLAR_DYES)
                    || item.has_tag(&tag::Item::C_DYES)
                {
                    if let Some(dye) = item_stack
                        .get_data_component::<pumpkin_data::data_component_impl::DyeImpl>()
                        && dye.color.id() != self.get_collar_color()
                    {
                        self.set_collar_color(dye.color.id());
                        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                        return true;
                    }
                } else if is_food
                    && self.mob_entity.living_entity.health.load()
                        < self.mob_entity.living_entity.get_max_health()
                {
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    self.mob_entity.living_entity.heal(2.0);
                    self.play_eating_sound();
                    return true;
                }

                let parent_interaction =
                    self.mob_entity
                        .mob_interact(player, item_stack, self.can_be_leashed());
                if !parent_interaction {
                    self.order_to_sit(!self.is_ordered_to_sit());
                    return true;
                }
                return parent_interaction;
            }
        } else if is_food {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.play_eating_sound();

            let rng = game_random();
            if rng.random_range(0..3) == 0 && self.approve_taming(player) {
                self.set_tame(true, Some(player.gameprofile.id));
                self.set_ordered_to_sit(true);
                self.get_entity().world.load().send_entity_status(
                    self.get_entity(),
                    EntityStatus::TamingSucceeded,
                    ActorEventID::TamingSucceeded,
                );
            } else {
                self.get_entity().world.load().send_entity_status(
                    self.get_entity(),
                    EntityStatus::TamingFailed,
                    ActorEventID::TamingFailed,
                );
            }

            return true;
        }

        self.mob_entity
            .mob_interact(player, item_stack, self.can_be_leashed())
    }

    /// The rest of vanilla `Cat.tick` after `super.tick()`: begging for fish and purring while
    /// lying down.
    fn post_tick(&self) {
        let entity = self.get_entity();
        let tick_count = entity.tick_count.load(Ordering::Relaxed);
        let is_baby = self.mob_entity.living_entity.is_baby();
        if !self.is_tame()
            && tick_count % 100 == 0
            && self.mob_entity.goals_selector.is_running::<CatTemptGoal>()
        {
            self.play_entity_sound(self.sound_variant().beg_for_food_sound(is_baby), 1.0, 1.0);
        }
        if (self.is_lying() || self.is_relax_state_one()) && tick_count % 5 == 0 {
            let rng = self.get_random();
            let volume = 0.6 + 0.4 * (rng.random::<f32>() - rng.random::<f32>());
            self.play_entity_sound(self.sound_variant().purr_sound(is_baby), volume, 1.0);
        }
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        !self.is_tame() && self.get_entity().tick_count.load(Ordering::Relaxed) > 2400
    }
}
