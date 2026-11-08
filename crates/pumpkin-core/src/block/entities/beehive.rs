use super::BlockEntity;
use crate::block::blocks::campfire::CampfireBlock;
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::mob::Mob;
use crate::entity::passive::bee::BeeEntity;
use crate::entity::player::Player;
use crate::entity::r#type::load_entity;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::World;
use crate::world::game_event::GameEventContext;
use pumpkin_data::block_properties::{BeeNestLikeProperties, BlockProperties};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId, HorizontalFacingExt};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::chunk::io::Dirtiable;
use pumpkin_world::world::BlockFlags;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Vanilla `emptyAllLivingFromHive`: released bees this close to the player target it.
const PLAYER_TARGET_DISTANCE_SQ: f64 = 16.0;
/// Vanilla `emptyAllLivingFromHive`: a sedated bee's `setStayOutOfHiveCountdown`.
const SEDATED_STAY_OUT_TICKS: i32 = 400;
/// Vanilla `BeehiveBlockEntity.MAX_OCCUPANTS`.
pub const MAX_OCCUPANTS: usize = 3;
/// Vanilla `BeehiveBlockEntity.MIN_OCCUPATION_TICKS_NECTAR`.
const MIN_OCCUPATION_TICKS_NECTAR: i32 = 2400;
/// Vanilla `BeehiveBlockEntity.MIN_OCCUPATION_TICKS_NECTARLESS`.
pub const MIN_OCCUPATION_TICKS_NECTARLESS: i32 = 600;
/// Vanilla `BeehiveBlock.MAX_HONEY_LEVELS`.
pub const MAX_HONEY_LEVELS: u8 = 5;

/// Vanilla `BeehiveBlockEntity.BeeReleaseStatus`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BeeReleaseStatus {
    HoneyDelivered,
    BeeReleased,
    Emergency,
}

/// Vanilla `BeehiveBlockEntity.BeeData` with its `Occupant`.
struct BeeData {
    entity_data: NbtCompound,
    ticks_in_hive: i32,
    min_ticks_in_hive: i32,
}

impl BeeData {
    fn from_tag(tag: &NbtTag) -> Option<Self> {
        let occupant = tag.extract_compound()?;
        Some(Self {
            entity_data: occupant
                .get_compound("entity_data")
                .or_else(|| occupant.get_compound("EntityData"))
                .cloned()
                .unwrap_or_default(),
            ticks_in_hive: occupant
                .get_int("ticks_in_hive")
                .or_else(|| occupant.get_int("TicksInHive"))
                .unwrap_or(0),
            min_ticks_in_hive: occupant
                .get_int("min_ticks_in_hive")
                .or_else(|| occupant.get_int("MinOccupationTicks"))
                .unwrap_or(0),
        })
    }

    fn to_tag(&self) -> NbtTag {
        let mut occupant = NbtCompound::new();
        occupant.put_compound("entity_data", self.entity_data.clone());
        occupant.put_int("ticks_in_hive", self.ticks_in_hive);
        occupant.put_int("min_ticks_in_hive", self.min_ticks_in_hive);
        NbtTag::Compound(occupant)
    }

    /// Vanilla `BeeData.tick`: whether the bee has stayed long enough to leave.
    const fn tick(&mut self) -> bool {
        let ticks = self.ticks_in_hive;
        self.ticks_in_hive += 1;
        ticks > self.min_ticks_in_hive
    }

    fn has_nectar(&self) -> bool {
        self.entity_data.get_bool("HasNectar").unwrap_or(false)
    }
}

pub struct BeehiveBlockEntity {
    pub position: BlockPos,
    stored: Mutex<Vec<BeeData>>,
    pub flower_pos: Mutex<Option<BlockPos>>,
}

impl BlockEntity for BeehiveBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let stored = nbt
            .get_list("bees")
            .or_else(|| nbt.get_list("Bees"))
            .map(|bees| bees.iter().filter_map(BeeData::from_tag).collect())
            .unwrap_or_default();
        let flower_pos = nbt
            .get_int_array("flower_pos")
            .and_then(|arr| (arr.len() == 3).then(|| BlockPos::new(arr[0], arr[1], arr[2])))
            .or_else(|| {
                nbt.get_compound("flower_pos")
                    .or_else(|| nbt.get_compound("FlowerPos"))
                    .map(|c| {
                        BlockPos::new(
                            c.get_int("X").or_else(|| c.get_int("x")).unwrap_or(0),
                            c.get_int("Y").or_else(|| c.get_int("y")).unwrap_or(0),
                            c.get_int("Z").or_else(|| c.get_int("z")).unwrap_or(0),
                        )
                    })
            });
        Self {
            position,
            stored: Mutex::new(stored),
            flower_pos: Mutex::new(flower_pos),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_list("bees", self.bee_tags());
        if let Ok(fp) = self.flower_pos.lock()
            && let Some(fp) = fp.as_ref()
        {
            nbt.put(
                "flower_pos",
                pumpkin_nbt::tag::NbtTag::IntArray(vec![fp.0.x, fp.0.y, fp.0.z]),
            );
        }
    }

    /// Vanilla `serverTick`.
    fn tick(&self, world: &Arc<World>) {
        let state_id = world.get_block_state_id(&self.position);
        self.tick_occupants(world, state_id);
        if !self.is_empty() && game_random().random::<f64>() < 0.005 {
            let pos = self.position.0;
            world.play_sound(
                Sound::BlockBeehiveWork,
                SoundCategory::Blocks,
                &Vector3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y),
                    f64::from(pos.z) + 0.5,
                ),
            );
        }
    }

    fn collect_item_components(&self, stack: &mut pumpkin_data::item_stack::ItemStack) {
        let bees = self.bee_tags();
        if !bees.is_empty() {
            stack.set_data_component(pumpkin_data::data_component_impl::BeesImpl { bees });
        }
    }

    fn apply_item_components(&self, stack: &pumpkin_data::item_stack::ItemStack) {
        if let Some(bees) =
            stack.get_data_component::<pumpkin_data::data_component_impl::BeesImpl>()
            && !bees.bees.is_empty()
        {
            *self.stored_bees() = bees.bees.iter().filter_map(BeeData::from_tag).collect();
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Vanilla `BeehiveBlockEntity.IGNORED_BEE_TAGS`: dropped from an occupant before it is loaded.
const IGNORED_BEE_TAGS: [&str; 25] = [
    "Air",
    "drop_chances",
    "equipment",
    "Brain",
    "CanPickUpLoot",
    "DeathTime",
    "fall_distance",
    "FallFlying",
    "Fire",
    "HurtTime",
    "LeftHanded",
    "Motion",
    "NoGravity",
    "OnGround",
    "PortalCooldown",
    "Pos",
    "Rotation",
    "sleeping_pos",
    "CannotEnterHiveTicks",
    "TicksSincePollination",
    "CropsGrownSincePollination",
    "hive_pos",
    "Passengers",
    "leash",
    "UUID",
];

impl BeehiveBlockEntity {
    pub const ID: &'static str = "minecraft:beehive";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            stored: Mutex::new(Vec::new()),
            flower_pos: Mutex::new(None),
        }
    }

    fn stored_bees(&self) -> std::sync::MutexGuard<'_, Vec<BeeData>> {
        self.stored
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn bee_tags(&self) -> Vec<NbtTag> {
        self.stored_bees().iter().map(BeeData::to_tag).collect()
    }

    /// Vanilla `isEmpty`.
    pub fn is_empty(&self) -> bool {
        self.stored_bees().is_empty()
    }

    /// Vanilla `isFull`.
    pub fn is_full(&self) -> bool {
        self.stored_bees().len() == MAX_OCCUPANTS
    }

    /// Vanilla `getOccupantCount`.
    pub fn occupant_count(&self) -> usize {
        self.stored_bees().len()
    }

    /// Vanilla `isFireNearby`: fire in the 3x3x3 cube around the hive.
    pub fn is_fire_nearby(&self, world: &World) -> bool {
        (-1..=1).any(|x| {
            (-1..=1).any(|y| {
                (-1..=1).any(|z| {
                    world.get_block(&self.position.offset(Vector3::new(x, y, z))) == &Block::FIRE
                })
            })
        })
    }

    /// Vanilla `isSedated`.
    pub fn is_sedated(&self, world: &World) -> bool {
        CampfireBlock::is_smokey_pos(world, &self.position)
    }

    /// Vanilla `Occupant.create`: a bee that has spent `ticks_in_hive` ticks in a new hive.
    #[must_use]
    pub fn new_bee_occupant(ticks_in_hive: i32) -> NbtTag {
        let mut entity_data = NbtCompound::new();
        entity_data.put_string("id", "minecraft:bee".to_string());
        BeeData {
            entity_data,
            ticks_in_hive,
            min_ticks_in_hive: MIN_OCCUPATION_TICKS_NECTARLESS,
        }
        .to_tag()
    }

    /// Vanilla `addOccupant`: the bee goes into the hive, unless it is full.
    pub fn add_occupant(&self, world: &Arc<World>, bee: &BeeEntity) {
        if self.stored_bees().len() >= MAX_OCCUPANTS {
            return;
        }
        let entity = bee.get_entity();
        if let Some(vehicle) = entity.get_vehicle() {
            vehicle.get_entity().remove_passenger(entity.entity_id);
        }
        entity.eject_passengers();
        if entity.is_leashed() {
            entity.drop_leash();
        }
        // Vanilla `Occupant.of`.
        let mut entity_data = NbtCompound::new();
        bee.write_nbt(&mut entity_data);
        for tag in IGNORED_BEE_TAGS {
            entity_data.child_tags.remove(tag);
        }
        let min_ticks_in_hive = if bee.has_nectar() {
            MIN_OCCUPATION_TICKS_NECTAR
        } else {
            MIN_OCCUPATION_TICKS_NECTARLESS
        };
        self.stored_bees().push(BeeData {
            entity_data,
            ticks_in_hive: 0,
            min_ticks_in_hive,
        });
        if let Some(bee_flower) = bee.saved_flower_pos() {
            let mut flower_pos = self
                .flower_pos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if flower_pos.is_none() || game_random().random::<bool>() {
                *flower_pos = Some(bee_flower);
            }
        }
        // Vanilla plays it at the hive's corner, not its centre.
        world.play_sound(
            Sound::BlockBeehiveEnter,
            SoundCategory::Blocks,
            &self.position.to_f64(),
        );
        world.emit_game_event_with_context(
            GameEvent::BlockChange.name(),
            self.position.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(bee),
            },
        );
        entity.remove();
        self.mark_chunk_dirty(world);
    }

    fn mark_chunk_dirty(&self, world: &World) {
        let _ = world
            .level
            .read_chunk_sync(&self.position.chunk_position(), |chunk| {
                chunk.mark_dirty(true);
            });
    }

    /// Vanilla `tickOccupants`: bees that stayed long enough leave, delivering their honey.
    fn tick_occupants(&self, world: &Arc<World>, state_id: BlockStateId) {
        let flower_pos = *self
            .flower_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        let mut stored = self.stored_bees();
        stored.retain_mut(|data| {
            if !data.tick() {
                return true;
            }
            let status = if data.has_nectar() {
                BeeReleaseStatus::HoneyDelivered
            } else {
                BeeReleaseStatus::BeeReleased
            };
            if Self::release_occupant(world, self.position, state_id, data, status, flower_pos)
                .is_some()
            {
                changed = true;
                return false;
            }
            true
        });
        drop(stored);
        if changed {
            self.mark_chunk_dirty(world);
        }
    }

    /// Vanilla `emptyAllLivingFromHive`: released bees within four blocks of the player go for
    /// it, unless smoke sedates the hive.
    pub fn empty_all_living_from_hive(
        &self,
        world: &Arc<World>,
        state_id: BlockStateId,
        player: Option<&Arc<Player>>,
        status: BeeReleaseStatus,
    ) {
        let released = self.release_all_occupants(world, state_id, status);
        let Some(player) = player else {
            return;
        };
        for entity in released {
            let Some(bee) = entity.cast_any().downcast_ref::<BeeEntity>() else {
                continue;
            };
            if player
                .position()
                .squared_distance_to_vec(&entity.get_entity().pos.load())
                > PLAYER_TARGET_DISTANCE_SQ
            {
                continue;
            }
            if self.is_sedated(world) {
                bee.cannot_enter_hive_ticks
                    .store(SEDATED_STAY_OUT_TICKS, Ordering::Relaxed);
            } else {
                bee.set_mob_target(Some(player.clone()), TargetReason::ClosestPlayer);
            }
        }
    }

    /// Vanilla `releaseAllOccupants`.
    fn release_all_occupants(
        &self,
        world: &Arc<World>,
        state_id: BlockStateId,
        status: BeeReleaseStatus,
    ) -> Vec<Arc<dyn EntityBase>> {
        let flower_pos = *self
            .flower_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut released = Vec::new();
        self.stored_bees().retain(|data| {
            let Some(entity) =
                Self::release_occupant(world, self.position, state_id, data, status, flower_pos)
            else {
                return true;
            };
            released.push(entity);
            false
        });
        if !released.is_empty() {
            self.mark_chunk_dirty(world);
        }
        released
    }

    /// Vanilla `releaseOccupant` and `Occupant.createEntity`; the bee when it left. Unless it is
    /// an emergency, bees stay in when the environment keeps them in or the hive's front is
    /// blocked. Honey goes up from `state_id`, the hive's state when its tick began.
    fn release_occupant(
        world: &Arc<World>,
        pos: BlockPos,
        state_id: BlockStateId,
        data: &BeeData,
        status: BeeReleaseStatus,
        flower_pos: Option<BlockPos>,
    ) -> Option<Arc<dyn EntityBase>> {
        let emergency = status == BeeReleaseStatus::Emergency;
        if world.bees_stay_in_hive(&pos) && !emergency {
            return None;
        }
        let props = BeeNestLikeProperties::from_state_id(state_id);
        let facing = props.facing.to_block_direction().to_offset();
        let front = pos.offset(facing);
        let front_blocked = !world.get_block_state(&front).collision_shapes.is_empty();
        if front_blocked && !emergency {
            return None;
        }
        let mut entity_nbt = data.entity_data.clone();
        for tag in IGNORED_BEE_TAGS {
            entity_nbt.child_tags.remove(tag);
        }
        let entity_type = entity_nbt
            .get_string("id")
            .and_then(|id| EntityType::from_name(id.strip_prefix("minecraft:").unwrap_or(id)))?;
        if !entity_type.has_tag(&tag::EntityType::MINECRAFT_BEEHIVE_INHABITORS) {
            return None;
        }
        let entity = load_entity(entity_type, pos.to_centered_f64(), &entity_nbt, world);
        let base = entity.get_entity();
        base.set_has_no_gravity(true);
        if let Some(bee) = entity.cast_any().downcast_ref::<BeeEntity>() {
            bee.set_hive_pos(Some(pos));
            // Vanilla `Occupant.setBeeReleaseData`.
            let ticks_in_hive = data.ticks_in_hive;
            if !bee.is_age_locked() {
                let age = bee.get_age();
                if age < 0 {
                    bee.set_age((age + ticks_in_hive).min(0));
                } else if age > 0 {
                    bee.set_age((age - ticks_in_hive).max(0));
                }
            }
            let love = &bee.mob_entity.love_ticks;
            love.store(
                (love.load(Ordering::Relaxed) - ticks_in_hive).max(0),
                Ordering::Relaxed,
            );

            if let Some(flower_pos) = flower_pos
                && bee.saved_flower_pos().is_none()
                && game_random().random::<f32>() < 0.9
            {
                bee.set_saved_flower_pos(Some(flower_pos));
            }
            if status == BeeReleaseStatus::HoneyDelivered {
                bee.drop_off_nectar();
                let block = Block::from_state_id(state_id);
                if block.has_tag(&tag::Block::MINECRAFT_BEEHIVES)
                    && <BeeNestLikeProperties as BlockProperties>::handles_block_id(block.id)
                    && props.honey_level < MAX_HONEY_LEVELS
                {
                    let mut increase = if game_random().random_range(0..100) == 0 {
                        2
                    } else {
                        1
                    };
                    if props.honey_level + increase > MAX_HONEY_LEVELS {
                        increase -= 1;
                    }
                    let mut new_props = props;
                    new_props.honey_level += increase;
                    world.set_block_state(
                        &pos,
                        new_props.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    );
                }
            }

            let delta = if front_blocked {
                0.0
            } else {
                0.55 + f64::from(base.width()) / 2.0
            };
            base.set_pos(Vector3::new(
                f64::from(pos.0.x) + 0.5 + delta * f64::from(facing.x),
                f64::from(pos.0.y) + 0.5 - f64::from(base.height()) / 2.0,
                f64::from(pos.0.z) + 0.5 + delta * f64::from(facing.z),
            ));
        }
        world.play_block_sound(Sound::BlockBeehiveExit, SoundCategory::Blocks, pos);
        world.emit_game_event_with_context(
            GameEvent::BlockChange.name(),
            pos.to_centered_f64(),
            &GameEventContext {
                source_entity: Some(entity.as_ref()),
            },
        );
        world.spawn_entity(entity.clone()).then_some(entity)
    }
}
