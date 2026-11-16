use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;

use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::mob::raider::create_ominous_banner;
use crate::entity::mob::spawn::EntitySpawnReason;

pub struct PatrolData {
    pub patrol_target: AtomicCell<Option<BlockPos>>,
    pub patrol_leader: AtomicBool,
    pub patrolling: AtomicBool,
}

impl Default for PatrolData {
    fn default() -> Self {
        Self {
            patrol_target: AtomicCell::new(None),
            patrol_leader: AtomicBool::new(false),
            patrolling: AtomicBool::new(false),
        }
    }
}

/// Vanilla `PatrollingMonster.finalizeSpawn`: the chance a mob spawned outside patrols, raids
/// and structures leads a patrol.
const LEADER_CHANCE: f32 = 0.06;

pub trait PatrollingMonster: Mob {
    fn get_patrol_data(&self) -> &PatrolData;

    fn can_be_leader(&self) -> bool {
        true
    }

    fn is_patrol_leader(&self) -> bool {
        self.get_patrol_data().patrol_leader.load(Ordering::Relaxed)
    }

    fn set_patrol_leader(&self, is_leader: bool) {
        self.get_patrol_data()
            .patrol_leader
            .store(is_leader, Ordering::Relaxed);
        self.set_patrolling(true);
    }

    fn is_patrolling(&self) -> bool {
        self.get_patrol_data().patrolling.load(Ordering::Relaxed)
    }

    fn set_patrolling(&self, patrolling: bool) {
        self.get_patrol_data()
            .patrolling
            .store(patrolling, Ordering::Relaxed);
    }

    fn get_patrol_target(&self) -> Option<BlockPos> {
        self.get_patrol_data().patrol_target.load()
    }

    fn set_patrol_target(&self, target: BlockPos) {
        self.get_patrol_data().patrol_target.store(Some(target));
        self.set_patrolling(true);
    }

    fn has_patrol_target(&self) -> bool {
        self.get_patrol_data().patrol_target.load().is_some()
    }

    fn can_join_patrol(&self) -> bool {
        true
    }

    /// Vanilla `PatrollingMonster.findPatrolTarget`: somewhere within 500 blocks.
    fn find_patrol_target(&self) {
        let block_pos = self.get_mob_entity().living_entity.entity.block_pos.load();
        let random = self.get_random();
        let dx = -500 + random.random_range(0..1000);
        let dz = -500 + random.random_range(0..1000);
        self.set_patrol_target(BlockPos(Vector3::new(
            block_pos.0.x + dx,
            block_pos.0.y,
            block_pos.0.z + dz,
        )));
    }

    /// Vanilla `PatrollingMonster.finalizeSpawn`: some mobs spawned outside patrols, raids and
    /// structures become leaders, and a leader wears the ominous banner and always drops it.
    fn finalize_patrol_spawn(&self, reason: EntitySpawnReason) {
        if !matches!(
            reason,
            EntitySpawnReason::Patrol | EntitySpawnReason::Event | EntitySpawnReason::Structure
        ) && self.get_random().random::<f32>() < LEADER_CHANCE
            && self.can_be_leader()
        {
            self.get_patrol_data()
                .patrol_leader
                .store(true, Ordering::Relaxed);
        }

        if self.is_patrol_leader() {
            let mob_entity = self.get_mob_entity();
            mob_entity.set_item_slot(&EquipmentSlot::HEAD, create_ominous_banner());
            mob_entity.set_guaranteed_drop(&EquipmentSlot::HEAD);
        }

        if reason == EntitySpawnReason::Patrol {
            self.set_patrolling(true);
        }
    }

    fn write_patrol_nbt(&self, nbt: &mut NbtCompound) {
        let data = self.get_patrol_data();
        if let Some(target) = data.patrol_target.load() {
            nbt.put(
                "patrol_target",
                NbtTag::IntArray(vec![target.0.x, target.0.y, target.0.z]),
            );
        }
        nbt.put_bool("PatrolLeader", data.patrol_leader.load(Ordering::Relaxed));
        nbt.put_bool("Patrolling", data.patrolling.load(Ordering::Relaxed));
    }

    fn read_patrol_nbt(&self, nbt: &NbtCompound) {
        let data = self.get_patrol_data();
        // Pumpkin used to save the target as three separate ints.
        let target = match nbt.get_int_array("patrol_target") {
            Some(&[x, y, z]) => Some(BlockPos::new(x, y, z)),
            _ => match (
                nbt.get_int("PatrolTargetX"),
                nbt.get_int("PatrolTargetY"),
                nbt.get_int("PatrolTargetZ"),
            ) {
                (Some(x), Some(y), Some(z)) => Some(BlockPos::new(x, y, z)),
                _ => None,
            },
        };
        data.patrol_target.store(target);
        data.patrol_leader.store(
            nbt.get_bool("PatrolLeader").unwrap_or(false),
            Ordering::Relaxed,
        );
        data.patrolling.store(
            nbt.get_bool("Patrolling").unwrap_or(false),
            Ordering::Relaxed,
        );
    }
}

/// Vanilla `PatrollingMonster.LongDistancePatrolGoal`: the leader walks towards its patrol
/// target ten blocks at a time and sends the members within 16 blocks to the same spot.
pub struct LongDistancePatrolGoal {
    speed_modifier: f64,
    leader_speed_modifier: f64,
    cooldown_until: i64,
}

impl LongDistancePatrolGoal {
    const NAVIGATION_FAILED_COOLDOWN: i64 = 200;
    const COMPANION_RANGE: f64 = 16.0;
    const LEG_LENGTH: f64 = 10.0;

    #[must_use]
    pub const fn new(speed_modifier: f64, leader_speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            leader_speed_modifier,
            cooldown_until: -1,
        }
    }

    fn game_time(mob: &dyn Mob) -> i64 {
        mob.get_entity()
            .world
            .load()
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .query_gametime()
    }

    /// Vanilla `findPatrolCompanions`: other patrolling monsters within 16 blocks that can join.
    fn find_patrol_companions(mob: &dyn Mob) -> Vec<std::sync::Arc<dyn crate::entity::EntityBase>> {
        let entity = mob.get_entity();
        entity
            .world
            .load()
            .get_entities_of_types_at_box(
                &entity.bounding_box.load().expand(
                    Self::COMPANION_RANGE,
                    Self::COMPANION_RANGE,
                    Self::COMPANION_RANGE,
                ),
                crate::entity::mob::raider::is_raider,
            )
            .into_iter()
            .filter(|other| {
                other.get_entity().entity_id != entity.entity_id
                    && other
                        .get_mob()
                        .and_then(|other| other.as_patrolling_monster())
                        .is_some_and(PatrollingMonster::can_join_patrol)
            })
            .collect()
    }

    /// The first air block above the motion-blocking surface, vanilla `getHeightmapPos`.
    fn surface(mob: &dyn Mob, x: i32, z: i32) -> BlockPos {
        let y = mob.get_entity().world.load().get_heightmap_height(
            ChunkHeightmapType::MotionBlockingNoLeaves,
            x,
            z,
        ) + 1;
        BlockPos::new(x, y, z)
    }

    fn move_to(mob: &dyn Mob, pos: BlockPos, speed: f64) -> bool {
        let steered = mob.get_mob_entity().steered();
        steered.navigator().move_to_coords(
            f64::from(pos.0.x),
            f64::from(pos.0.y),
            f64::from(pos.0.z),
            speed,
            &steered.mob_entity().living_entity,
        )
    }

    /// Vanilla `moveRandomly`: a step to a random surface spot within 8 blocks.
    fn move_randomly(&self, mob: &dyn Mob) {
        let pos = mob.get_entity().block_pos.load();
        let random = mob.get_random();
        let x = pos.0.x - 8 + random.random_range(0..16);
        let z = pos.0.z - 8 + random.random_range(0..16);
        Self::move_to(mob, Self::surface(mob, x, z), self.speed_modifier);
    }
}

impl Goal for LongDistancePatrolGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(patrol) = mob.as_patrolling_monster() else {
            return false;
        };
        patrol.is_patrolling()
            && mob.get_mob_entity().get_target().is_none()
            && mob.get_controlling_passenger().is_none()
            && patrol.has_patrol_target()
            && Self::game_time(mob) >= self.cooldown_until
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(patrol) = mob.as_patrolling_monster() else {
            return;
        };
        let navigation_done = mob.get_mob_entity().steered().navigator().is_done();
        if !navigation_done {
            return;
        }
        let Some(patrol_target) = patrol.get_patrol_target() else {
            return;
        };
        let is_leader = patrol.is_patrol_leader();
        let companions = Self::find_patrol_companions(mob);
        let pos = mob.get_entity().pos.load();
        if patrol.is_patrolling() && companions.is_empty() {
            patrol.set_patrolling(false);
        } else if is_leader
            && patrol_target
                .to_centered_f64()
                .squared_distance_to_vec(&pos)
                < Self::LEG_LENGTH * Self::LEG_LENGTH
        {
            patrol.find_patrol_target();
        } else {
            let target = Vector3::new(
                f64::from(patrol_target.0.x) + 0.5,
                f64::from(patrol_target.0.y),
                f64::from(patrol_target.0.z) + 0.5,
            );
            // Vanilla passes 90 to `Vec3.yRot`, which takes radians.
            let (sin, cos) = 90.0f32.sin_cos();
            let offset = pos - target;
            let rotated = Vector3::new(
                offset.x * f64::from(cos) + offset.z * f64::from(sin),
                offset.y,
                offset.z * f64::from(cos) - offset.x * f64::from(sin),
            );
            let aim = rotated * 0.4 + target;
            let move_target = (aim - pos).normalize() * Self::LEG_LENGTH + pos;
            let path_target = BlockPos::floored_v(move_target);
            let path_target = Self::surface(mob, path_target.0.x, path_target.0.z);
            let speed = if is_leader {
                self.leader_speed_modifier
            } else {
                self.speed_modifier
            };
            if !Self::move_to(mob, path_target, speed) {
                self.move_randomly(mob);
                self.cooldown_until = Self::game_time(mob) + Self::NAVIGATION_FAILED_COOLDOWN;
            } else if is_leader {
                for companion in companions {
                    if let Some(companion) = companion
                        .get_mob()
                        .and_then(|companion| companion.as_patrolling_monster())
                    {
                        companion.set_patrol_target(path_target);
                    }
                }
            }
        }
    }
}
