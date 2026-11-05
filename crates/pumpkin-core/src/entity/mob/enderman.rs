use crate::entity::breath::DROWNING_INTERVAL;
use crate::entity::mob::AuditedTeleport;
use crate::entity::r#type::CreatureSpawnLevel;
use crate::world::World;
use crate::world::brightness::DAYLIGHT_BRIGHTNESS;
use crate::world::entity_islands::HurtReach;
use crate::world::game_event::GameEventContext;
use pumpkin_data::game_rules::{GameRule, GameRuleValue};
use pumpkin_world::chunk::ChunkHeightmapType::MotionBlocking;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::ai::behavior::hunting::{self, Hunt};
use crate::entity::ai::target_match::TargetMatch;
use crate::entity::attributes::Modifier;
use crate::entity::attributes::ModifierOperation;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockState, BlockStateId, Enchantment, attributes::Attributes};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    entity::{EntityStatus, EntityType},
    game_event::GameEvent,
    sound::{Sound, SoundCategory},
    tag,
    tag::Taggable,
    world::WorldEvent,
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_world::generation::structure::template::PaletteEntry;

use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{
            chase_player::ChasePlayerGoal, look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
            pick_up_block::PickUpBlockGoal, place_block::PlaceBlockGoal,
            reset_universal_anger::ResetUniversalAngerGoal, revenge::RevengeGoal, swim::SwimGoal,
            teleport_towards_player::TeleportTowardsPlayerGoal, wander_around::WanderAroundGoal,
        },
        pathfinder::node::PathType,
    },
    mob::{
        Mob, MobEntity,
        neutral::{NeutralData, NeutralMob},
    },
    player::Player,
};

const SPEED_BOOST: f64 = 0.15;
/// Vanilla `Enderman.MAX_TELEPORT_RADIUS`: `teleport_randomly` moves at most this far on each
/// horizontal axis.
const RANDOM_TELEPORT_RANGE: f64 = 32.0;
/// `teleport_randomly` aims at most this far above the enderman.
const RANDOM_TELEPORT_RISE: i32 = 31;
/// Vanilla `Enderman.MIN_DEAGGRESSION_TIME`: ticks after a target change before daylight can
/// send the enderman away.
const MIN_DEAGGRESSION_TIME: i32 = 600;
/// Vanilla `Entity.checkBelowWorld`: the void hurts this far below the world.
const OUT_OF_WORLD_DEPTH: i32 = 64;
const ENDERMAN_SPEED_BOOST_ID: &str = "minecraft:attacking";

pub const ENDERMAN_EYE_HEIGHT: f64 = 2.55;
pub const PLAYER_EYE_HEIGHT: f64 = 1.62;

fn is_projectile_damage(dt: DamageType) -> bool {
    let (names, _) = pumpkin_data::tag::DamageType::MINECRAFT_IS_PROJECTILE;
    names.contains(&dt.message_id)
}

const ENDERMAN_HUNTS: &[Hunt] = &[Hunt::new(
    3,
    TargetMatch::Type(&EntityType::ENDERMITE),
    true,
)];

pub struct EndermanEntity {
    pub mob_entity: MobEntity,
    neutral_data: NeutralData,
    carried_block: AtomicCell<Option<BlockStateId>>,
    /// Synced CREEPY flag, the open-mouth pose. Anger lives in `NeutralMob`.
    creepy: AtomicBool,
    provoked: AtomicBool,
    speed_boosted: AtomicBool,
    /// Vanilla `targetChangeTime`: the tick count of the last target set, 0 without a target.
    target_change_time: AtomicI32,
}

impl EndermanEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let entity = Self {
            mob_entity,
            neutral_data: NeutralData::default(),
            carried_block: AtomicCell::new(None),
            creepy: AtomicBool::new(false),
            provoked: AtomicBool::new(false),
            speed_boosted: AtomicBool::new(false),
            target_change_time: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(entity);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        let mut navigator = mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
        drop(navigator);

        {
            let goal_selector = &mob_arc.mob_entity.goals_selector;
            let target_selector = &mob_arc.mob_entity.target_selector;

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(ChasePlayerGoal::new(mob_arc.clone())));
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(1.0, false)));
            goal_selector.add_goal(
                7,
                Box::new(WanderAroundGoal::water_avoiding_with_probability(1.0, 0.0)),
            );
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(10, Box::new(PlaceBlockGoal::new(mob_arc.clone())));
            goal_selector.add_goal(11, Box::new(PickUpBlockGoal::new(mob_arc.clone())));

            target_selector.add_goal(1, Box::new(TeleportTowardsPlayerGoal::new(mob_arc.clone())));
            target_selector.add_goal(2, Box::new(RevengeGoal::new(true)));
            hunting::apply(target_selector, &mob_arc.mob_entity, ENDERMAN_HUNTS);
            target_selector.add_goal(4, ResetUniversalAngerGoal::new(false));
        };

        mob_arc
    }

    /// Vanilla `Enderman.teleport()`.
    pub fn teleport_randomly(&self) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return false;
        }
        let pos = entity.pos.load();
        let (x, y, z) = {
            let rng = self.get_random();
            (
                pos.x + (rng.random_range(0.0..1.0) - 0.5) * 2.0 * RANDOM_TELEPORT_RANGE,
                pos.y + f64::from(rng.random_range(0i32..64) - 32),
                pos.z + (rng.random_range(0.0..1.0) - 0.5) * 2.0 * RANDOM_TELEPORT_RANGE,
            )
        };

        self.teleport_to(x, y, z)
    }

    /// Vanilla `Enderman.teleportTowards`.
    pub fn teleport_towards(&self, target: &dyn EntityBase) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        let target = target.get_entity();
        let target_pos = target.pos.load();
        let dir = Vector3::new(
            pos.x - target_pos.x,
            pos.y + f64::from(entity.height()) * 0.5 - target.get_eye_y(),
            pos.z - target_pos.z,
        );
        // `Vec3.normalize` leaves a near-zero vector at zero.
        let length = dir.length();
        let dir = if length < 1.0E-5 {
            Vector3::default()
        } else {
            dir * (1.0 / length)
        };
        let (x, y, z) = {
            let rng = self.get_random();
            (
                pos.x + (rng.random_range(0.0..1.0) - 0.5) * 8.0 - dir.x * 16.0,
                pos.y + f64::from(rng.random_range(0i32..16) - 8) - dir.y * 16.0,
                pos.z + (rng.random_range(0.0..1.0) - 0.5) * 8.0 - dir.z * 16.0,
            )
        };

        self.teleport_to(x, y, z)
    }

    /// Vanilla `Enderman.teleport(x, y, z)`.
    pub fn teleport_to(&self, x: f64, y: f64, z: f64) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.has_vehicle() {
            return false;
        }
        let origin = entity.pos.load();
        if !self.random_teleport(x, y, z) {
            return false;
        }
        let world = entity.world.load();
        world.emit_game_event_with_context(
            GameEvent::Teleport.name(),
            origin,
            &GameEventContext {
                source_entity: Some(self),
            },
        );
        if !entity.is_silent() {
            let old_block = BlockPos::containing_vec(origin);
            world.sync_world_event(
                WorldEvent::ParticlesEndermanTeleport,
                old_block,
                old_block.clamped_pack_difference_in_position(
                    &entity.block_pos.load(),
                    127,
                    127,
                    127,
                ),
            );
            world.play_sound(
                Sound::EntityEndermanTeleport,
                SoundCategory::Hostile,
                &origin,
            );
            self.play_entity_sound(Sound::EntityEndermanTeleport, 1.0, 1.0);
        }
        true
    }

    /// Vanilla `LivingEntity.randomTeleport` with `Enderman.canRandomlyTeleportTo` and the
    /// `enderman_does_not_teleport_to` tag.
    fn random_teleport(&self, x: f64, y: f64, z: f64) -> bool {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let Some(new_pos) = living.random_teleport_target(
            x,
            y,
            z,
            |block| block.has_tag(&tag::Block::MINECRAFT_ENDERMAN_DOES_NOT_TELEPORT_TO),
            |world, pos| {
                !world
                    .get_fluid(&BlockPos::containing_vec(pos).down())
                    .has_tag(&tag::Fluid::MINECRAFT_WATER)
            },
        ) else {
            return false;
        };
        let world = entity.world.load();
        let origin = entity.pos.load();
        if let Some(server) = world.server.upgrade() {
            let mut event =
                crate::plugin::api::events::entity::entity_teleport::EntityTeleportEvent::new(
                    entity.entity_id,
                    origin,
                    new_pos,
                );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return false;
            }
        }

        entity.teleport(new_pos, None, None, &world);
        world.send_entity_status(entity, EntityStatus::Teleport, ActorEventID::None);
        self.mob_entity.steered().navigator().stop();
        true
    }

    /// Vanilla `Enderman.repeatedlyTryToTeleport`.
    fn repeatedly_try_to_teleport(&self) {
        if self.mob_entity.living_entity.entity.has_vehicle() {
            return;
        }
        for _ in 0..64 {
            if self.teleport_randomly() {
                return;
            }
        }
    }

    /// Where `teleports` chained random teleports can put the body: up to 32 blocks across and
    /// 31 up per teleport, and down to any ground below.
    pub fn teleport_area(&self, teleports: u32) -> BoundingBox {
        let entity = self.get_entity();
        let world = entity.world.load();
        let teleports = f64::from(teleports);
        let mut area = entity.bounding_box.load().expand(
            RANDOM_TELEPORT_RANGE * teleports,
            0.0,
            RANDOM_TELEPORT_RANGE * teleports,
        );
        area.min.y = f64::from(world.get_bottom_y());
        area.max.y = f64::from(RANDOM_TELEPORT_RISE)
            .mul_add(teleports, entity.pos.load().y)
            .min(f64::from(world.get_top_y()))
            + f64::from(entity.height());
        area
    }

    pub fn set_target(&self, target: Option<Arc<dyn EntityBase>>) {
        let is_some = target.is_some();
        let mut mob_target = self
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *mob_target = target;
        drop(mob_target);

        if is_some {
            self.target_change_time.store(
                self.get_entity().tick_count.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.set_creepy(true);
            // Use attribute modifier instead of direct speed arithmetic
            if !self.speed_boosted.swap(true, Ordering::Relaxed) {
                let living = &self.mob_entity.living_entity;
                let modifier = Modifier {
                    id: ENDERMAN_SPEED_BOOST_ID.to_string(),
                    amount: SPEED_BOOST,
                    operation: ModifierOperation::Add,
                };

                living.update_attribute(&Attributes::MOVEMENT_SPEED, |inst| {
                    inst.add_or_replace_modifier(modifier);
                });

                crate::entity::attributes::send_attribute_updates_for_living(
                    living,
                    vec![Attributes::MOVEMENT_SPEED],
                );
            }
        } else {
            self.target_change_time.store(0, Ordering::Relaxed);
            self.set_creepy(false);
            self.set_provoked(false);
            if self.speed_boosted.swap(false, Ordering::Relaxed) {
                let living = &self.mob_entity.living_entity;

                living.update_attribute(&Attributes::MOVEMENT_SPEED, |inst| {
                    inst.remove_modifier(ENDERMAN_SPEED_BOOST_ID);
                });

                crate::entity::attributes::send_attribute_updates_for_living(
                    living,
                    vec![Attributes::MOVEMENT_SPEED],
                );
            }
        }
    }

    pub fn set_creepy(&self, creepy: bool) {
        self.creepy.store(creepy, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::enderman::CREEPY, creepy);
    }

    pub fn is_creepy(&self) -> bool {
        self.creepy.load(Ordering::Relaxed)
    }

    pub fn set_provoked(&self, provoked: bool) {
        self.provoked.store(provoked, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::enderman::STARED_AT, provoked);
    }

    pub fn set_carried_block(&self, block_state: Option<BlockStateId>) {
        self.carried_block.store(block_state);
        let value = block_state.map_or(VarInt(0), |id| VarInt(id.as_u16() as i32));
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::enderman::CARRY_STATE, value);
    }

    pub fn get_carried_block(&self) -> Option<BlockStateId> {
        self.carried_block.load()
    }

    pub fn is_player_staring(&self, player: &Player) -> bool {
        let equipment = player.living_entity.entity_equipment.try_lock();
        if let Ok(equipment) = equipment
            && let Some(head_stack) = equipment.equipment.get(&EquipmentSlot::HEAD)
            && !head_stack.is_empty()
            && head_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_GAZE_DISGUISE_EQUIPMENT)
        {
            return false;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let enderman_pos = entity.pos.load();
        let enderman_eye_y = enderman_pos.y + ENDERMAN_EYE_HEIGHT;

        let player_entity = player.get_entity();
        let player_pos = player_entity.pos.load();
        let player_eye_y = player_pos.y + PLAYER_EYE_HEIGHT;

        let pitch = player_entity.pitch.load().to_radians();
        let yaw = -player_entity.yaw.load().to_radians();

        let cos_pitch = pitch.cos();
        let look_dir = Vector3::new(
            (yaw.sin() * cos_pitch) as f64,
            (-pitch.sin()) as f64,
            (yaw.cos() * cos_pitch) as f64,
        );

        let dx = enderman_pos.x - player_pos.x;
        let dy = enderman_eye_y - player_eye_y;
        let dz = enderman_pos.z - player_pos.z;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();

        if distance < 0.1 {
            return false;
        }

        let dir_x = dx / distance;
        let dir_y = dy / distance;
        let dir_z = dz / distance;

        let dot = look_dir.x * dir_x + look_dir.y * dir_y + look_dir.z * dir_z;

        if dot <= 1.0 - 0.025 / distance {
            return false;
        }

        // Vanilla `isLookingAtMe(..., seeThroughTransparentBlocks = false, ...)`: the player's
        // collider line of sight, so endermen aren't provoked through glass.
        (player as &dyn EntityBase).has_line_of_sight(entity)
    }
}

impl NeutralMob for EndermanEntity {
    fn get_neutral_data(&self) -> &NeutralData {
        &self.neutral_data
    }
}

impl Mob for EndermanEntity {
    /// Vanilla `Enderman.dropCustomDeathLoot`: the carried block's own drops, mined with a
    /// silk touch diamond axe (`enderman_loot_drop`).
    fn drop_custom_death_loot(&self) {
        let Some(state_id) = self.get_carried_block() else {
            return;
        };
        let entity = self.get_entity();
        let world = entity.world.load();
        let block = Block::from_state_id(state_id);
        let Some(loot_table) = block.loot_table.and_then(|key| world.get_loot_table(key)) else {
            return;
        };
        let mut tool = ItemStack::new(1, &Item::DIAMOND_AXE);
        tool.add_enchantment(&Enchantment::SILK_TOUCH, 1);
        let params = LootContextParameters {
            world: Some(&world),
            block_state: Some(BlockState::from_id(state_id)),
            position: Some(entity.pos.load()),
            tool: Some(tool),
            this_entity: Some(self),
            ..Default::default()
        };
        for stack in generate_loot_from_handle(&loot_table, 0, &params) {
            entity.spawn_at_location(stack);
        }
    }

    /// Vanilla `Enderman.getWalkTargetValue`.
    fn get_walk_target_value(
        &self,
        _level: &dyn CreatureSpawnLevel,
        _pos: &pumpkin_util::math::position::BlockPos,
    ) -> f32 {
        0.0
    }

    fn as_neutral(&self) -> Option<&dyn NeutralMob> {
        Some(self)
    }

    fn is_sensitive_to_water(&self) -> bool {
        true
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(block_state) = self.carried_block.load() {
            nbt.put("carriedBlockState", PaletteEntry::state_to_nbt(block_state));
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let block_state = match nbt.get("carriedBlockState") {
            // Older Pumpkin saves stored the raw state id.
            Some(NbtTag::Int(id)) => Some(BlockStateId::new(*id as u16)),
            Some(tag) => Some(PaletteEntry::state_from_nbt(tag)),
            None => None,
        };
        if let Some(block_state) = block_state {
            self.set_carried_block(block_state);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_metadata(&self) -> Option<crate::entity::mob::MobMetadata> {
        self.is_creepy()
            .then(|| crate::entity::mob::MobMetadata::new().screaming(true))
    }

    fn apply_mob_target(&self, target: Option<Arc<dyn EntityBase>>) {
        self.set_target(target);
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_creepy() {
            Sound::EntityEndermanScream
        } else {
            Sound::EntityEndermanAmbient
        })
    }

    fn requires_custom_persistence(&self) -> bool {
        self.get_carried_block().is_some()
    }

    /// Vanilla `Enderman.customServerAiStep`: daylight sends a calm enderman away.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if !world.is_bright_outside()
            || entity.tick_count.load(Ordering::Relaxed)
                < self.target_change_time.load(Ordering::Relaxed) + MIN_DEAGGRESSION_TIME
        {
            return;
        }
        let brightness = light_level_dependent_magic_value(entity);
        if brightness > DAYLIGHT_BRIGHTNESS
            && world.can_see_sky(&entity.block_pos.load())
            && self.get_random().random::<f32>() * 30.0 < (brightness - 0.4) * 2.0
        {
            self.set_mob_target(None, TargetReason::ForgotTarget);
            self.teleport_randomly();
        }
    }

    /// Vanilla `Enderman.hurtServer`: projectiles and thrown potions make it teleport away, and
    /// every hurt not caused by a living entity can teleport it, even when it takes no damage.
    fn mob_hurt_server(
        &self,
        caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        let living = &self.mob_entity.living_entity;
        if living.entity.is_invulnerable_to(&damage_type, cause) {
            return Some(false);
        }
        if is_projectile_damage(damage_type) && !living.entity.has_vehicle() {
            self.repeatedly_try_to_teleport();
            return Some(false);
        }
        // Thrown potions only hurt it through `hurt_water_sensitive_entities`, the water branch
        // of `hurtWithCleanWater`.
        let thrown_potion = source.is_some_and(|source| {
            let kind = source.get_entity().entity_type;
            kind == &EntityType::SPLASH_POTION || kind == &EntityType::LINGERING_POTION
        });
        let damaged =
            living.damage_with_context(caller, amount, damage_type, position, source, cause);
        if thrown_potion {
            self.repeatedly_try_to_teleport();
        } else if cause
            .or(source)
            .is_none_or(|attacker| attacker.get_living_entity().is_none())
            && self.get_random().random_range(0..10) != 0
        {
            self.teleport_randomly();
        }
        Some(damaged)
    }

    fn has_audited_tick(&self) -> bool {
        true
    }

    /// The look-for-player goal, daylight and the water or rain hurt at the end of the tick each
    /// teleport it at most once, each from where the one before put it. Hurts from what it
    /// touches can come once per block, so those ticks run alone.
    fn audited_teleport(
        &self,
        moved: &BoundingBox,
        goal_teleport: Option<BoundingBox>,
    ) -> AuditedTeleport {
        if self.may_be_hurt_by_surroundings(moved) {
            return AuditedTeleport::Unbounded;
        }
        let mut teleports = u32::from(goal_teleport.is_some());
        if self.may_teleport_from_daylight(teleports > 0) {
            teleports += 1;
        }
        // A teleport can put it in the rain or next to water.
        if teleports > 0 || may_be_wet(&self.get_entity().world.load(), moved) {
            teleports += 1;
        }
        if teleports == 0 {
            AuditedTeleport::None
        } else {
            AuditedTeleport::Within(self.teleport_area(teleports))
        }
    }

    // A hit teleports it once, from where it stands.
    fn hurt_reach(&self) -> Option<HurtReach> {
        Some(HurtReach {
            area: self.teleport_area(1),
            changes_blocks: false,
        })
    }
}

impl EndermanEntity {
    /// `custom_server_ai_step` can teleport it this tick. `moved_before` means a teleport earlier
    /// in the tick can change the light and sky it sees.
    fn may_teleport_from_daylight(&self, moved_before: bool) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        if !world.is_bright_outside() {
            return false;
        }
        // Dropping the target this tick resets the change time.
        let tick = entity.tick_count.load(Ordering::Relaxed) + 1;
        if self.mob_entity.get_target().is_none()
            && tick < self.target_change_time.load(Ordering::Relaxed) + MIN_DEAGGRESSION_TIME
        {
            return false;
        }
        moved_before
            || (light_level_dependent_magic_value(entity) > DAYLIGHT_BRIGHTNESS
                && world.can_see_sky(&entity.block_pos.load()))
    }

    /// Whether something it can touch this tick in `moved` can hurt it without a living
    /// attacker: fire, lava, freezing, drowning, a fall, a wall, cramming, the void or a
    /// damaging block.
    fn may_be_hurt_by_surroundings(&self, moved: &BoundingBox) -> bool {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let world = entity.world.load();
        let fall = moved.max.y - entity.bounding_box.load().max.y;
        if entity.fire_ticks.load(Ordering::Relaxed) > 0
            || entity.frozen_ticks.load(Ordering::Relaxed) > 0
            || entity.air_supply.load(Ordering::Relaxed) <= 1 - DROWNING_INTERVAL
            || living.fall_distance.load() + fall
                >= living.get_attribute_value(&Attributes::SAFE_FALL_DISTANCE)
            || entity.is_in_wall()
            || moved.min.y < f64::from(world.get_bottom_y() - OUT_OF_WORLD_DEPTH)
        {
            return true;
        }
        if let GameRuleValue::Int(max_cramming) = world.get_game_rule(&GameRule::MaxEntityCramming)
            && max_cramming > 0
            && world.get_all_at_box(moved).len() as i64 > max_cramming
        {
            return true;
        }
        // Blocks it walks in or steps on.
        let contact = moved.expand_towards(0.0, -1.0, 0.0);
        BlockPos::iterate(contact.min_block_pos(), contact.max_block_pos()).any(|pos| {
            let block = world.get_block(&pos);
            HURTING_BLOCKS.iter().any(|hurting| hurting.id == block.id)
                || world.get_fluid(&pos).has_tag(&tag::Fluid::MINECRAFT_LAVA)
        })
    }
}

/// Blocks whose contact, step or landing callbacks hurt it.
const HURTING_BLOCKS: [&Block; 11] = [
    &Block::FIRE,
    &Block::SOUL_FIRE,
    &Block::CAMPFIRE,
    &Block::SOUL_CAMPFIRE,
    &Block::MAGMA_BLOCK,
    &Block::CACTUS,
    &Block::SWEET_BERRY_BUSH,
    &Block::WITHER_ROSE,
    &Block::POINTED_DRIPSTONE,
    &Block::POWDER_SNOW,
    &Block::LAVA_CAULDRON,
];

/// Whether water or rain can reach a body in `moved` at the end of the tick, when vanilla
/// `aiStep` hurts water-sensitive mobs.
fn may_be_wet(world: &World, moved: &BoundingBox) -> bool {
    let (min, max) = (moved.min_block_pos().0, moved.max_block_pos().0);
    if world.is_raining()
        && (min.x..=max.x).any(|x| {
            (min.z..=max.z).any(|z| world.get_heightmap_height(MotionBlocking, x, z) < max.y)
        })
    {
        return true;
    }
    BlockPos::iterate(moved.min_block_pos(), moved.max_block_pos())
        .any(|pos| world.get_fluid(&pos).has_tag(&tag::Fluid::MINECRAFT_WATER))
}

/// Vanilla `Entity.getLightLevelDependentMagicValue`: the light at the eyes, 0 in an unloaded
/// column.
fn light_level_dependent_magic_value(entity: &Entity) -> f32 {
    let world = entity.world.load();
    let eye = BlockPos::containing_vec(entity.get_eye_pos());
    if world
        .level
        .read_chunk_sync(&eye.chunk_position(), |_| ())
        .is_none()
    {
        return 0.0;
    }
    world.get_light_level_dependent_magic_value(&eye)
}
