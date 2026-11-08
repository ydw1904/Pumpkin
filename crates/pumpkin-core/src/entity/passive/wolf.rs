use crossbeam::atomic::AtomicCell;
use pumpkin_util::random::gameplay::game_random;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use pumpkin_data::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::wolf_sound_variant::WolfSoundVariant;
use pumpkin_data::wolf_variant::WolfVariant;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;

use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::spawn::SpawnGroupData;
use crate::entity::passive::sheep::get_mixed_color;
use crate::entity::{
    Entity, EntityBase,
    ageable::AgeableMob,
    ai::behavior::{
        fleeing::{self, Flee},
        hunting::{self, Hunt},
        neutral::apply_targets,
    },
    ai::goal::{
        active_target::TargetCondition, beg::BegGoal, breed::BreedGoal,
        escape_danger::EscapeDangerGoal, follow_owner::FollowOwnerGoal,
        leap_at_target::LeapAtTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        owner_hurt_by_target::OwnerHurtByTargetGoal, owner_hurt_target::OwnerHurtTargetGoal,
        revenge::RevengeGoal, sit_when_ordered_to::SitWhenOrderedToGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    ai::target_match::TargetMatch,
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    passive::{
        animal::Animal,
        llama::scares_wolves,
        tamable::{TamableAnimal, TamableData, is_untamed},
    },
    player::Player,
};
use crate::plugin::api::events::entity::entity_target::TargetReason;

const WOLF_PREY: TargetMatch =
    TargetMatch::AnyOf(&[&EntityType::SHEEP, &EntityType::RABBIT, &EntityType::FOX]);

const WOLF_FLEES: &[Flee] = &[Flee::new(3, TargetMatch::LLAMA_LIKE, 24.0, 1.5, 1.5)
    .when(is_untamed)
    .confirmed_by(scares_wolves)
    .dropping_target()];

const WOLF_HUNTS: &[Hunt] = &[
    Hunt::new(5, WOLF_PREY, false).when(TargetCondition::Untamed),
    hunting::baby_turtles(6, false).when(TargetCondition::Untamed),
    Hunt::new(7, TargetMatch::SKELETON_LIKE, false),
];

pub struct WolfEntity {
    pub mob_entity: MobEntity,
    pub variant: AtomicU8,
    pub sound_variant: AtomicU8,
    pub collar_color: AtomicU8,
    pub tamable_data: TamableData,
    pub ageable_data: crate::entity::ageable::AgeableData,
    pub neutral_data: NeutralData,
    /// Vanilla `isWet`: got wet and hasn't shaken it off yet.
    is_wet: AtomicBool,
    is_shaking: AtomicBool,
    /// Vanilla `shakeAnim` and `shakeAnimO`, which time the shake.
    shake_anim: AtomicCell<f32>,
    shake_anim_o: AtomicCell<f32>,
}

/// The `MAX_HEALTH` base values `Wolf.applyTamingSideEffects` sets.
const TAME_MAX_HEALTH: f32 = 40.0;
const WILD_MAX_HEALTH: f32 = 8.0;
/// Vanilla `Wolf.getSoundVolume`.
const WOLF_SOUND_VOLUME: f32 = 0.4;
/// `shakeAnim` gained per tick; the shake ends once it passes `SHAKE_END`.
const SHAKE_STEP: f32 = 0.05;
const SHAKE_END: f32 = 2.0;

impl WolfEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Wolf.defineSynchedData`.
        crate::entity::passive::tamable::define_tamable_animal_synched_data(&entity);
        entity.synched_data.define(
            pumpkin_data::tracked_data::wolf::DATA_VARIANT_ID,
            VarInt(i32::from(
                pumpkin_data::wolf_variant::WolfVariant::Pale.id(),
            )),
        );
        entity.synched_data.define(
            pumpkin_data::tracked_data::wolf::DATA_SOUND_VARIANT_ID,
            VarInt(i32::from(
                pumpkin_data::wolf_sound_variant::WolfSoundVariant::Classic.id(),
            )),
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::wolf::DATA_INTERESTED_ID, false);
        entity.synched_data.define(
            pumpkin_data::tracked_data::wolf::DATA_COLLAR_COLOR,
            VarInt(pumpkin_data::dye_color::DyeColor::Red as i32),
        );
        entity
            .synched_data
            .define(pumpkin_data::tracked_data::wolf::DATA_ANGER_END_TIME, -1i64);
        let mob_entity = MobEntity::new(entity);
        let wolf = Self {
            mob_entity,
            variant: AtomicU8::new(WolfVariant::Pale.id()),
            sound_variant: AtomicU8::new(WolfSoundVariant::Classic as u8),
            collar_color: AtomicU8::new(14), // Default to red
            tamable_data: TamableData::default(),
            ageable_data: crate::entity::ageable::AgeableData::default(),
            neutral_data: NeutralData::default(),
            is_wet: AtomicBool::new(false),
            is_shaking: AtomicBool::new(false),
            shake_anim: AtomicCell::new(0.0),
            shake_anim_o: AtomicCell::new(0.0),
        };
        let mob_arc = Arc::new(wolf);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;

            // Goal selector (matching Vanilla registerGoals):
            // 1: SwimGoal (FloatGoal)
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            // 1: TamableAnimalPanicGoal
            goal_selector.add_goal(
                1,
                EscapeDangerGoal::tamable(
                    1.5,
                    &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES,
                ),
            );
            // 2: SitWhenOrderedToGoal
            goal_selector.add_goal(2, Box::new(SitWhenOrderedToGoal::new()));
            // 3: WolfAvoidEntityGoal (Llama)
            fleeing::apply(goal_selector, WOLF_FLEES);
            // 4: LeapAtTargetGoal
            goal_selector.add_goal(4, Box::new(LeapAtTargetGoal::new(0.4)));
            // 5: MeleeAttackGoal
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.0, true)));
            // 6: FollowOwnerGoal
            goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 2.0));
            // 7: BreedGoal
            goal_selector.add_goal(7, BreedGoal::new(1.0));
            // 8: WaterAvoidingRandomStrollGoal
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::water_avoiding(1.0)));
            // 9: BegGoal
            goal_selector.add_goal(9, BegGoal::new(8.0));
            // 10: LookAtPlayer & RandomLookAround
            goal_selector.add_goal(
                10,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let target_selector = &mob_arc.mob_entity.target_selector;

            // Target selector (matching Vanilla registerGoals):
            // 1: OwnerHurtByTargetGoal
            target_selector.add_goal(1, OwnerHurtByTargetGoal::new());
            // 2: OwnerHurtTargetGoal
            target_selector.add_goal(2, OwnerHurtTargetGoal::new());
            // 3: HurtByTargetGoal (RevengeGoal)
            target_selector.add_goal(3, Box::new(RevengeGoal::new(true).alerting_others()));
            // 4: NearestAttackableTarget (Player, angry only), 8: ResetUniversalAnger
            apply_targets(target_selector, &mob_arc.mob_entity, 4, 8, true);
            // 5, 6: NonTameRandomTarget (prey, baby turtle), 7: NearestAttackableTarget (AbstractSkeleton)
            hunting::apply(target_selector, &mob_arc.mob_entity, WOLF_HUNTS);
        };

        mob_arc
    }

    fn stop_and_forget_target(&self) {
        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        self.set_mob_target(None, TargetReason::ForgotTarget);
    }

    fn reset_shake(&self) {
        self.is_shaking.store(false, Ordering::Relaxed);
        self.shake_anim.store(0.0);
        self.shake_anim_o.store(0.0);
    }

    /// The server part of vanilla `Wolf.aiStep` and `Wolf.tick`: a wet wolf that stands still on
    /// the ground shakes itself dry, unless it gets wet again first.
    fn tick_shake(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if self.is_wet.load(Ordering::Relaxed)
            && !self.is_shaking.load(Ordering::Relaxed)
            && self.is_navigator_idle()
            && entity.on_ground.load(Ordering::Relaxed)
        {
            self.reset_shake();
            self.is_shaking.store(true, Ordering::Relaxed);
            world.send_entity_status(entity, EntityStatus::ShakeWetness, ActorEventID::None);
        }
        if !entity.is_alive() {
            return;
        }
        if entity.is_in_water_or_rain() {
            self.is_wet.store(true, Ordering::Relaxed);
            if self.is_shaking.load(Ordering::Relaxed) {
                world.send_entity_status(
                    entity,
                    EntityStatus::CancelShakeWetness,
                    ActorEventID::None,
                );
                self.reset_shake();
            }
        } else if self.is_shaking.load(Ordering::Relaxed) {
            if self.shake_anim.load() == 0.0 {
                let pitch = {
                    let rng = self.get_random();
                    (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 1.0)
                };
                self.play_entity_sound(Sound::EntityWolfShake, WOLF_SOUND_VOLUME, pitch);
                entity.emit_own_game_event(self, GameEvent::EntityAction);
            }
            let previous = self.shake_anim.load();
            self.shake_anim_o.store(previous);
            self.shake_anim.store(previous + SHAKE_STEP);
            if previous >= SHAKE_END {
                self.is_wet.store(false, Ordering::Relaxed);
                self.reset_shake();
            }
        }
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
}

impl AgeableMob for WolfEntity {
    fn get_ageable_data(&self) -> &crate::entity::ageable::AgeableData {
        &self.ageable_data
    }
}

impl Animal for WolfEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .get_item()
            .has_tag(&tag::Item::MINECRAFT_WOLF_FOOD)
    }

    /// Vanilla `Wolf.getBreedOffspring`: either parent's variant, a tame parent's owner with the
    /// collars mixed, and a random sound variant.
    fn get_breed_offspring(&self, partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let baby = Self::new(Entity::new(
            world.clone(),
            entity.pos.load(),
            &EntityType::WOLF,
        ));
        if let Some(partner) = partner.cast_any().downcast_ref::<Self>() {
            let random = self.get_random();
            let parent = if random.random::<bool>() {
                self
            } else {
                partner
            };
            baby.set_variant_id(parent.variant.load(Ordering::Relaxed));
            if self.is_tame() {
                baby.set_owner(self.get_owner());
                baby.set_tame(true);
                baby.apply_taming_side_effects();
                let collar = get_mixed_color(
                    &world,
                    DyeColor::from(self.collar_color.load(Ordering::Relaxed)),
                    DyeColor::from(partner.collar_color.load(Ordering::Relaxed)),
                );
                baby.set_collar_color(collar.id());
            }
            let sound_variants = WolfSoundVariant::ALL;
            baby.set_sound_variant(sound_variants[random.random_range(0..sound_variants.len())]);
        }
        baby
    }
}

impl TamableAnimal for WolfEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }

    /// Vanilla `Wolf.applyTamingSideEffects`.
    fn apply_taming_side_effects(&self) {
        let living = &self.mob_entity.living_entity;
        if self.is_tame() {
            living.set_max_health(TAME_MAX_HEALTH);
            living.set_health(TAME_MAX_HEALTH);
        } else {
            living.set_max_health(WILD_MAX_HEALTH);
        }
    }
}

impl NeutralMob for WolfEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }

    fn anger_end_time_data(&self) -> Option<pumpkin_data::tracked_data::TrackedData> {
        Some(pumpkin_data::tracked_data::wolf::DATA_ANGER_END_TIME)
    }
}

impl Mob for WolfEntity {
    /// Vanilla `Wolf.finalizeSpawn`: a pack shares the variant its first wolf picked from the
    /// spawn biome, and every wolf rolls its own sound variant.
    fn finalize_spawn(
        &self,
        world: &Arc<crate::world::World>,
        _reason: crate::entity::mob::spawn::EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let random = self.get_random();
        let variant = if let Some(SpawnGroupData::WolfPack(variant)) = group_data {
            Some(variant)
        } else {
            let biome = crate::world::natural_spawner::spawn_biome(
                world,
                &self.get_entity().block_pos.load(),
            );
            WolfVariant::select_to_spawn(biome.registry_id, |count| random.random_range(0..count))
        };
        let sound_variants = WolfSoundVariant::ALL;
        self.set_sound_variant(sound_variants[random.random_range(0..sound_variants.len())]);
        let Some(variant) = variant else {
            return self.finalize_spawn_super(group_data);
        };
        self.set_variant(variant);
        // `AgeableMob.finalizeSpawn` with a group that spawns no babies.
        self.mob_entity.finalize_spawn_base();
        Some(SpawnGroupData::WolfPack(variant))
    }

    fn get_ambient_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        let is_baby = self.is_baby();
        let sound_variant = WolfSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        Some(if self.is_angry() {
            sound_variant.growl_sound(is_baby)
        } else if self.get_random().random_range(0..3) == 0 {
            if self.is_tame() && self.mob_entity.living_entity.health.load() < 20.0 {
                sound_variant.whine_sound(is_baby)
            } else {
                sound_variant.pant_sound(is_baby)
            }
        } else {
            sound_variant.ambient_sound(is_baby)
        })
    }

    /// Vanilla `Wolf.hurtServer`: any hurt it isn't invulnerable to makes it stand up, even one
    /// the hurt cooldown then blocks.
    fn pre_damage_with_context(
        &self,
        damage_type: DamageType,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if !self.get_entity().is_invulnerable_to(&damage_type, cause) {
            self.order_to_sit(false);
        }
        true
    }

    fn post_tick(&self) {
        self.tick_shake();
    }

    // Shaking only touches the wolf; owner teleports come from its goals.
    fn has_audited_tick(&self) -> bool {
        true
    }

    fn on_die(&self, _source: Option<&dyn EntityBase>) {
        self.is_wet.store(false, Ordering::Relaxed);
        self.reset_shake();
    }

    fn can_shear_equipment(&self, player: &Player) -> bool {
        self.is_owned_by(&player.gameprofile.id)
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {
        let sound_variant = WolfSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        self.play_entity_sound(sound_variant.step_sound(self.is_baby()), 0.15, 1.0);
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        let mut meta = crate::entity::mob::MobMetadata::new();
        if self.is_baby() {
            meta = meta.baby(true);
        }
        if self.is_tame() {
            meta = meta.tamed(true);
        }
        if self.is_in_sitting_pose() {
            meta = meta.sitting(true);
        }
        if self.is_angry() {
            meta = meta.angry(true);
        }
        meta = meta.collar_color(
            pumpkin_data::tracked_data::wolf::COLLAR_COLOR,
            pumpkin_data::dye_color::DyeColor::from(self.collar_color.load(Ordering::Relaxed)),
        );
        Some(meta)
    }

    fn can_attack_with_owner(&self, target: &dyn EntityBase, owner: &dyn EntityBase) -> bool {
        let target_entity = target.get_entity();
        let target_type = target_entity.entity_type;
        if *target_type == EntityType::CREEPER
            || *target_type == EntityType::GHAST
            || *target_type == EntityType::ARMOR_STAND
        {
            return false;
        }

        if *target_type == EntityType::WOLF {
            if let Some(target_mob) = target.get_mob()
                && let Some(tamable) = target_mob.as_tamable()
                && tamable.is_tame()
                && let Some(target_owner) = tamable.get_owner()
                && let Some(owner_player) = owner.get_player()
                && target_owner == owner_player.gameprofile.id
            {
                return false;
            }
            return true;
        }

        if *target_type == EntityType::PLAYER {
            if let Some(owner_player) = owner.get_player()
                && let Some(target_player) = target.get_player()
            {
                if owner_player.gameprofile.id == target_player.gameprofile.id {
                    return false;
                }
                let world = target_player.world();
                if !world.level_info.load().game_rules.pvp {
                    return false;
                }
            }
            return true;
        }

        !target.get_mob().is_some_and(Mob::is_tamed)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let variant_id = u32::from(self.variant.load(Ordering::Relaxed));
        let variant_str =
            self.resolve_dynamic_entry_id("minecraft:wolf_variant", variant_id, |id| {
                WolfVariant::all()
                    .get(id as usize)
                    .map_or("minecraft:pale", WolfVariant::asset_id)
                    .to_string()
            });
        nbt.put_string("variant", variant_str);
        let sound_variant_id = u32::from(self.sound_variant.load(Ordering::Relaxed));
        let sound_variant_str =
            self.resolve_dynamic_entry_id("minecraft:wolf_sound_variant", sound_variant_id, |id| {
                let sound_variant = WolfSoundVariant::from_id(id as u8).unwrap_or_default();
                format!("minecraft:{}", sound_variant.to_name())
            });
        nbt.put_string("sound_variant", sound_variant_str);
        nbt.put_byte(
            "CollarColor",
            self.collar_color.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant_str) = nbt.get_string("variant")
            && let Some(id) =
                self.resolve_dynamic_raw_id("minecraft:wolf_variant", variant_str, |n| {
                    WolfVariant::from_name(n).map(|v| u32::from(v.id()))
                })
        {
            self.set_variant_id(id as u8);
        }
        if let Some(sound_str) = nbt.get_string("sound_variant")
            && let Some(id) =
                self.resolve_dynamic_raw_id("minecraft:wolf_sound_variant", sound_str, |n| {
                    WolfSoundVariant::from_name(n).map(|v| v as u32)
                })
        {
            self.set_sound_variant_id(id as u8);
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

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(id) = self.resolve_dynamic_raw_id("minecraft:wolf_variant", name, |n| {
            WolfVariant::from_name(n).map(|v| u32::from(v.id()))
        }) {
            self.set_variant_id(id as u8);
        }
    }

    fn mob_set_sound_variant_name(&self, name: &str) {
        if let Some(id) = self.resolve_dynamic_raw_id("minecraft:wolf_sound_variant", name, |n| {
            WolfSoundVariant::from_name(n).map(|v| v as u32)
        }) {
            self.set_sound_variant_id(id as u8);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::wolf::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::TAMEABLE_FLAGS,
            self.get_tame_flags(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::COLLAR_COLOR,
            VarInt(self.collar_color.load(Ordering::Relaxed) as i32),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::WOLF_VARIANT_ID,
            VarInt(self.variant.load(Ordering::Relaxed) as i32),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::DATA_SOUND_VARIANT_ID,
            VarInt(self.sound_variant.load(Ordering::Relaxed) as i32),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::OWNER_UUID,
            self.get_owner(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();
        if self.is_tame() {
            if self.is_food(item_stack)
                && self.mob_entity.living_entity.health.load()
                    < self.mob_entity.living_entity.get_max_health()
            {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                self.mob_entity.living_entity.heal(2.0);
                return true;
            }

            if self.is_owned_by(&player.gameprofile.id) {
                if let Some(dye) =
                    item_stack.get_data_component::<pumpkin_data::data_component_impl::DyeImpl>()
                    && dye.color.id() != self.get_collar_color()
                {
                    self.set_collar_color(dye.color.id());
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    return true;
                }

                let parent_interaction = self.animal_interact(player, item_stack);
                if !parent_interaction {
                    if self.order_to_sit(!self.is_ordered_to_sit()) {
                        self.stop_and_forget_target();
                    }
                    return true;
                }
                return parent_interaction;
            }
        } else if item == &Item::BONE && !self.mob_entity.is_attacking() {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let rng = game_random();
            if rng.random_range(0..3) == 0 && self.approve_taming(player) {
                TamableAnimal::tame(self, player.gameprofile.id);
                self.stop_and_forget_target();
                self.set_ordered_to_sit(true);
                self.spawn_taming_particles(true);
            } else {
                self.spawn_taming_particles(false);
            }
            return true;
        }

        self.animal_interact(player, item_stack)
    }
}

impl CustomSound for WolfEntity {
    fn death_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        let is_baby = self.is_baby();
        let sound_variant = WolfSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        Some(sound_variant.death_sound(is_baby))
    }

    fn hurt_sound(&self) -> Option<pumpkin_data::sound::Sound> {
        let is_baby = self.is_baby();
        let sound_variant = WolfSoundVariant::from_id(self.sound_variant.load(Ordering::Relaxed))
            .unwrap_or_default();
        Some(sound_variant.hurt_sound(is_baby))
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }
}

impl WolfEntity {
    pub fn get_collar_color(&self) -> u8 {
        self.collar_color.load(Ordering::Relaxed)
    }

    pub fn set_collar_color(&self, color: u8) {
        self.collar_color.store(color, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::COLLAR_COLOR,
            VarInt(color as i32),
        );
    }

    pub fn set_variant(&self, variant: WolfVariant) {
        self.set_variant_id(variant.id());
    }

    pub fn set_variant_id(&self, raw_id: u8) {
        self.variant.store(raw_id, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::WOLF_VARIANT_ID,
            VarInt(i32::from(raw_id)),
        );
    }

    pub fn set_sound_variant(&self, sound_variant: WolfSoundVariant) {
        self.set_sound_variant_id(sound_variant as u8);
    }

    pub fn set_sound_variant_id(&self, raw_id: u8) {
        self.sound_variant.store(raw_id, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::wolf::DATA_SOUND_VARIANT_ID,
            VarInt(i32::from(raw_id)),
        );
    }
}
