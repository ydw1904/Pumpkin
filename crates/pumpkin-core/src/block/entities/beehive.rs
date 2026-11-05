use super::BlockEntity;
use crate::entity::ageable::AgeableMob;
use crate::entity::passive::bee::BeeEntity;
use crate::entity::r#type::load_entity;
use crate::world::World;
use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

pub struct BeehiveBlockEntity {
    pub position: BlockPos,
    pub bees: Mutex<Option<Vec<NbtTag>>>,
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
        let bees = nbt
            .get_list("bees")
            .or_else(|| nbt.get_list("Bees"))
            .map(<[_]>::to_vec);
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
            bees: Mutex::new(bees),
            flower_pos: Mutex::new(flower_pos),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Ok(b) = self.bees.lock()
            && let Some(b) = b.as_ref()
        {
            nbt.put_list("bees", b.clone());
        }
        if let Ok(fp) = self.flower_pos.lock()
            && let Some(fp) = fp.as_ref()
        {
            nbt.put(
                "flower_pos",
                pumpkin_nbt::tag::NbtTag::IntArray(vec![fp.0.x, fp.0.y, fp.0.z]),
            );
        }
    }

    fn collect_item_components(&self, stack: &mut pumpkin_data::item_stack::ItemStack) {
        if let Ok(bees) = self.bees.lock()
            && let Some(bees) = bees.as_ref()
            && !bees.is_empty()
        {
            stack.set_data_component(pumpkin_data::data_component_impl::BeesImpl {
                bees: bees.clone(),
            });
        }
    }

    fn apply_item_components(&self, stack: &pumpkin_data::item_stack::ItemStack) {
        if let Some(bees) =
            stack.get_data_component::<pumpkin_data::data_component_impl::BeesImpl>()
            && !bees.bees.is_empty()
        {
            *self
                .bees
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bees.bees.clone());
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
            bees: Mutex::new(None),
            flower_pos: Mutex::new(None),
        }
    }

    /// Vanilla `releaseAllOccupants` with `BeeReleaseStatus.BEE_RELEASED`: bees stay in when
    /// the environment keeps them in or the hive's front is blocked.
    pub fn release_all_occupants(&self, world: &Arc<World>, facing: HorizontalFacing) {
        let pos = self.position;
        if world.bees_stay_in_hive(&pos) {
            return;
        }
        let front = pos.offset(facing.to_block_direction().to_offset());
        if !world.get_block_state(&front).collision_shapes.is_empty() {
            return;
        }
        let mut bees = self
            .bees
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(occupants) = bees.as_mut() else {
            return;
        };
        occupants.retain(|occupant| !Self::release_occupant(world, pos, facing, occupant));
    }

    /// Vanilla `releaseOccupant` and `Occupant.createEntity`; true when the bee left.
    fn release_occupant(
        world: &Arc<World>,
        pos: BlockPos,
        facing: HorizontalFacing,
        occupant: &NbtTag,
    ) -> bool {
        let Some(occupant) = occupant.extract_compound() else {
            return false;
        };
        let mut entity_nbt = occupant
            .get_compound("entity_data")
            .cloned()
            .unwrap_or_default();
        for tag in IGNORED_BEE_TAGS {
            entity_nbt.child_tags.remove(tag);
        }
        let Some(entity_type) = entity_nbt
            .get_string("id")
            .and_then(|id| EntityType::from_name(id.strip_prefix("minecraft:").unwrap_or(id)))
        else {
            return false;
        };
        if !entity_type.has_tag(&tag::EntityType::MINECRAFT_BEEHIVE_INHABITORS) {
            return false;
        }
        let entity = load_entity(entity_type, pos.to_centered_f64(), &entity_nbt, world);
        let base = entity.get_entity();
        base.set_has_no_gravity(true);
        if let Some(bee) = entity.cast_any().downcast_ref::<BeeEntity>() {
            let ticks_in_hive = occupant.get_int("ticks_in_hive").unwrap_or(0);
            // Vanilla `Occupant.setBeeReleaseData`.
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
            let offset = facing.to_block_direction().to_offset();
            let delta = 0.55 + f64::from(base.width()) / 2.0;
            base.set_pos(Vector3::new(
                f64::from(pos.0.x) + 0.5 + delta * f64::from(offset.x),
                f64::from(pos.0.y) + 0.5 - f64::from(base.height()) / 2.0,
                f64::from(pos.0.z) + 0.5 + delta * f64::from(offset.z),
            ));
        }
        world.play_block_sound(Sound::BlockBeehiveExit, SoundCategory::Blocks, pos);
        world.emit_game_event(GameEvent::BlockChange.name(), pos.to_f64());
        world.spawn_entity(entity)
    }
}
