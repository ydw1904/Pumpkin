use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{cos, sin, wrap_degrees};
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::{chunk::ChunkHeightmapType, world::BlockFlags};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::{
    Entity, EntityBase,
    living::LivingEntity,
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::dragon_fight::DragonFight;
use crate::world::end_podium;

pub mod flight_history;
pub mod phase;

use flight_history::DragonFlightHistory;
use phase::{EnderDragonPhase, PhaseManager};

pub const NODE_COUNT: usize = 24;
/// The height vanilla's landing and takeoff phases look for nodes at.
pub const NODE_Y: i32 = 105;
const DEATH_TIME_MAX: i32 = 200;
const SITTING_ALLOWED_DAMAGE_PERCENTAGE: f32 = 0.25;

const NODE_ADJACENCY: [u32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057,
    3166214, 2138120, 6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712,
    8257536,
];

/// A pathfinding node of the dragon's flight graph, vanilla `Node`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct DragonNode {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl DragonNode {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn distance_to_sqr(&self, other: &Self) -> f32 {
        let xd = (other.x - self.x) as f32;
        let yd = (other.y - self.y) as f32;
        let zd = (other.z - self.z) as f32;
        xd * xd + yd * yd + zd * zd
    }

    #[must_use]
    pub fn distance_to(&self, other: &Self) -> f32 {
        self.distance_to_sqr(other).sqrt()
    }
}

/// Vanilla `Path` as the dragon uses it: a list of nodes and the index of the next one.
#[derive(Clone, Debug)]
pub struct DragonPath {
    nodes: Vec<DragonNode>,
    next_node_index: usize,
}

impl DragonPath {
    pub const fn advance(&mut self) {
        self.next_node_index += 1;
    }

    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.next_node_index >= self.nodes.len()
    }

    #[must_use]
    pub fn next_node_pos(&self) -> DragonNode {
        self.nodes[self.next_node_index]
    }
}

pub struct EnderDragonPart {
    pub entity: Entity,
    pub dragon_uuid: uuid::Uuid,
    /// The head and neck take full damage; every other part a reduced amount.
    pub head_or_neck: bool,
}

impl EnderDragonPart {
    pub const fn new(entity: Entity, dragon_uuid: uuid::Uuid, head_or_neck: bool) -> Self {
        Self {
            entity,
            dragon_uuid,
            head_or_neck,
        }
    }
}

impl EntityBase for EnderDragonPart {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn damage(&self, caller: &dyn EntityBase, amount: f32, damage_type: DamageType) -> bool {
        self.damage_with_context(caller, amount, damage_type, None, None, None)
    }

    /// Vanilla `EnderDragonPart.hurtServer`: the hit goes to the dragon, scaled by part.
    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let world = self.entity.world.load();
        let entities = world.entities.load();
        let Some(dragon_base) = entities
            .iter()
            .find(|e| e.get_entity().entity_uuid == self.dragon_uuid)
        else {
            return false;
        };
        let Some(dragon) = dragon_base.cast_any().downcast_ref::<EnderDragonEntity>() else {
            return false;
        };
        dragon.hurt(
            dragon_base.as_ref(),
            self.head_or_neck,
            amount,
            damage_type,
            position,
            source,
            cause,
        )
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn can_hit(&self) -> bool {
        true
    }
}

pub struct EnderDragonEntity {
    pub mob_entity: MobEntity,
    pub parts: Vec<Arc<EnderDragonPart>>,

    pub flight_history: Mutex<DragonFlightHistory>,
    pub flap_time: Mutex<f32>,
    pub o_flap_time: Mutex<f32>,

    pub in_wall: Mutex<bool>,
    pub dragon_death_time: Mutex<i32>,
    pub sitting_damage_received: Mutex<f32>,
    pub y_rot_a: Mutex<f32>,
    nearest_crystal: Mutex<Option<Arc<dyn EntityBase>>>,

    pub fight_origin: Mutex<BlockPos>,
    nodes: Mutex<Option<[DragonNode; NODE_COUNT]>>,

    pub phase_manager: PhaseManager,
    phase: Mutex<EnderDragonPhase>,
}

impl EnderDragonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        let base_id = entity.entity_id;
        let dragon_uuid = entity.entity_uuid;
        let world = entity.world.load();

        let _ = Entity::reserve_ids(8);

        let mut parts = Vec::new();
        for i in 1..=8 {
            let part_entity = Entity::from_uuid_with_id(
                base_id + i,
                uuid::Uuid::new_v4(),
                world.clone(),
                entity.pos.load(),
                &EntityType::ENDER_DRAGON,
            );
            // Parts 1 and 2 are the head and the neck.
            let part = Arc::new(EnderDragonPart::new(part_entity, dragon_uuid, i <= 2));
            world.add_entity_lookup_only(part.clone() as Arc<dyn EntityBase>);
            parts.push(part);
        }

        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            parts,
            flight_history: Mutex::new(DragonFlightHistory::default()),
            flap_time: Mutex::new(0.0),
            o_flap_time: Mutex::new(0.0),
            in_wall: Mutex::new(false),
            dragon_death_time: Mutex::new(0),
            sitting_damage_received: Mutex::new(0.0),
            y_rot_a: Mutex::new(0.0),
            nearest_crystal: Mutex::new(None),
            fight_origin: Mutex::new(BlockPos::new(0, 0, 0)),
            nodes: Mutex::new(None),
            phase_manager: PhaseManager::default(),
            // Vanilla's phase manager starts out hovering; the dragon fight switches its dragon
            // to the holding pattern.
            phase: Mutex::new(EnderDragonPhase::Hovering),
        })
    }

    pub fn set_fight_origin(&self, pos: BlockPos) {
        *self
            .fight_origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pos;
    }

    fn fight_origin(&self) -> BlockPos {
        *self
            .fight_origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn current_phase_type(&self) -> EnderDragonPhase {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn current_phase(&self) -> &dyn phase::Phase {
        self.phase_manager.get_phase(self.current_phase_type())
    }

    fn is_sitting(&self) -> bool {
        self.current_phase().is_sitting()
    }

    /// Vanilla `EnderDragonPhaseManager.setPhase`.
    pub fn set_phase(&self, phase_type: EnderDragonPhase) {
        let old_phase = {
            let mut phase = self
                .phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *phase == phase_type {
                return;
            }
            std::mem::replace(&mut *phase, phase_type)
        };

        self.phase_manager.get_phase(old_phase).end(self);
        self.get_entity().set_synced_data(
            tracked_data::ender_dragon::DATA_PHASE,
            VarInt(phase_type.id()),
        );
        self.phase_manager.get_phase(phase_type).begin(self);
    }

    #[must_use]
    pub fn get_pos(&self) -> Vector3<f64> {
        self.get_entity().pos.load()
    }

    #[must_use]
    pub fn collided(&self) -> bool {
        let entity = self.get_entity();
        entity.horizontal_collision.load(Ordering::Relaxed)
            || entity.vertical_collision.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn horizontal_speed(&self) -> f32 {
        self.get_entity().velocity.load().horizontal_length() as f32
    }

    #[must_use]
    pub fn head_pos(&self) -> Vector3<f64> {
        self.parts[0].entity.pos.load()
    }

    pub fn update_y_rot_a(&self, update: impl FnOnce(f32) -> f32) -> f32 {
        let mut y_rot_a = self
            .y_rot_a
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *y_rot_a = update(*y_rot_a);
        *y_rot_a
    }

    /// Runs `f` on the dragon fight, if this dragon is the fight's dragon (vanilla's
    /// `getDragonFight()` being non-null). The fight must not be locked by the caller.
    fn with_dragon_fight<R>(&self, f: impl FnOnce(&mut DragonFight) -> R) -> Option<R> {
        let world = self.get_entity().world.load();
        let fight = world.dragon_fight.as_ref()?;
        let mut fight = fight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (fight.dragon_uuid() == Some(self.get_entity().entity_uuid)).then(|| f(&mut fight))
    }

    /// The fight's crystal count, `None` without a fight.
    #[must_use]
    pub fn alive_crystals(&self) -> Option<i32> {
        self.with_dragon_fight(|fight| fight.alive_crystals())
    }

    /// The first free block above the exit portal podium, vanilla
    /// `level.getHeightmapPos(type, EnderDragonFight.getPodiumLocation(fightOrigin))`.
    #[must_use]
    pub fn podium_pos(&self, heightmap: ChunkHeightmapType) -> BlockPos {
        let podium = end_podium::get_location(self.fight_origin());
        let world = self.get_entity().world.load();
        let y = world.get_heightmap_height(heightmap, podium.0.x, podium.0.z) + 1;
        BlockPos::new(podium.0.x, y, podium.0.z)
    }

    /// Vanilla `Entity.calculateViewVector`.
    #[must_use]
    pub fn calculate_view_vector(x_rot: f32, y_rot: f32) -> Vector3<f64> {
        let x_rad = x_rot.to_radians();
        let y_rad = -y_rot.to_radians();
        let (y_cos, y_sin) = (cos(y_rad), sin(y_rad));
        let (x_cos, x_sin) = (cos(x_rad), sin(x_rad));
        Vector3::new(
            f64::from(y_sin * x_cos),
            f64::from(-x_sin),
            f64::from(y_cos * x_cos),
        )
    }

    /// Vanilla `EnderDragon.getHeadLookVector`.
    #[must_use]
    pub fn head_look_vector(&self) -> Vector3<f64> {
        let entity = self.get_entity();
        let x_rot = match self.current_phase_type() {
            EnderDragonPhase::Landing | EnderDragonPhase::Takeoff => {
                let egg = self.podium_pos(ChunkHeightmapType::MotionBlockingNoLeaves);
                let pos = self.get_pos();
                let dist =
                    ((egg.dist_to_center_sqr(pos.x, pos.y, pos.z).sqrt() as f32) / 4.0).max(1.0);
                let y_offset = 6.0 / dist;
                -y_offset * 1.5 * 5.0
            }
            _ if self.is_sitting() => -45.0,
            _ => entity.pitch.load(),
        };
        Self::calculate_view_vector(x_rot, entity.yaw.load())
    }

    /// The nearest player matching `targeting`, measured from `origin`, vanilla
    /// `level.getNearestPlayer(targeting, dragon, x, y, z)`.
    #[must_use]
    pub fn nearest_player(
        &self,
        origin: Vector3<f64>,
        targeting: &TargetPredicate,
    ) -> Option<Arc<Player>> {
        let world = self.get_entity().world.load();
        world.get_nearest_player(origin, f64::INFINITY, |player| {
            targeting.test(&world, Some(self), player.as_ref())
        })
    }

    /// Vanilla `EnderDragon.canAttack`.
    #[must_use]
    pub fn can_attack_player(&self, player: &Player) -> bool {
        player.can_be_seen_as_enemy()
    }

    /// Vanilla `findClosestNode()`, which also lays out the nodes on first use.
    pub fn find_closest_node(&self) -> usize {
        let pos = self.get_pos();
        self.find_closest_node_to(pos.x, pos.y, pos.z)
    }

    fn nodes(&self) -> [DragonNode; NODE_COUNT] {
        let mut nodes = self
            .nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *nodes.get_or_insert_with(|| {
            let world = self.get_entity().world.load();
            std::array::from_fn(|i| {
                // Three rings around the world origin: 12 nodes at 60 blocks, 8 at 40, 4 at 20.
                let (radius, count, index, y_adjustment) = match i {
                    0..12 => (60.0, 12.0, i, 5),
                    12..20 => (40.0, 8.0, i - 12, 15),
                    _ => (20.0, 4.0, i - 20, 5),
                };
                let angle =
                    2.0 * (-std::f32::consts::PI + std::f32::consts::PI / count * index as f32);
                let node_x = (radius * cos(angle)).floor() as i32;
                let node_z = (radius * sin(angle)).floor() as i32;
                let height = world.get_heightmap_height(
                    ChunkHeightmapType::MotionBlockingNoLeaves,
                    node_x,
                    node_z,
                ) + 1;
                DragonNode::new(node_x, 73.max(height + y_adjustment), node_z)
            })
        })
    }

    /// Vanilla `findClosestNode(x, y, z)`: only the inner rings once the crystals are gone.
    pub fn find_closest_node_to(&self, x: f64, y: f64, z: f64) -> usize {
        let nodes = self.nodes();
        let current = DragonNode::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
        let start_index = if self.alive_crystals().is_none_or(|crystals| crystals == 0) {
            12
        } else {
            0
        };

        let mut closest_dist = 10000.0;
        let mut closest_index = 0;
        for (i, node) in nodes.iter().enumerate().skip(start_index) {
            let dist = node.distance_to_sqr(&current);
            if dist < closest_dist {
                closest_dist = dist;
                closest_index = i;
            }
        }
        closest_index
    }

    /// Vanilla `EnderDragon.findPath`: A* over the node graph, ending at `final_node` if given.
    /// Falls back to the node closest to the end when it's unreachable.
    pub fn find_path(
        &self,
        start: usize,
        end: usize,
        final_node: Option<DragonNode>,
    ) -> Option<DragonPath> {
        let nodes = self.nodes();
        let minimum_index = if self.alive_crystals().is_none_or(|crystals| crystals == 0) {
            12
        } else {
            0
        };

        let to = nodes[end];
        let mut g = [0.0f32; NODE_COUNT];
        let mut f = [0.0f32; NODE_COUNT];
        let mut closed = [false; NODE_COUNT];
        let mut came_from = [None; NODE_COUNT];
        let mut open: Vec<usize> = vec![start];
        f[start] = nodes[start].distance_to(&to);
        let mut closest = start;

        let reconstruct = |came_from: &[Option<usize>; NODE_COUNT], last: usize| {
            let mut path = vec![nodes[last]];
            let mut node = last;
            while let Some(previous) = came_from[node] {
                path.push(nodes[previous]);
                node = previous;
            }
            path.reverse();
            path.extend(final_node);
            DragonPath {
                nodes: path,
                next_node_index: 0,
            }
        };

        while let Some(open_index) =
            (0..open.len()).min_by(|&a, &b| f[open[a]].total_cmp(&f[open[b]]))
        {
            let current = open.swap_remove(open_index);
            if nodes[current] == to {
                return Some(reconstruct(&came_from, current));
            }
            if nodes[current].distance_to(&to) < nodes[closest].distance_to(&to) {
                closest = current;
            }
            closed[current] = true;

            for adjacent in minimum_index..NODE_COUNT {
                if NODE_ADJACENCY[current] & (1 << adjacent) == 0 || closed[adjacent] {
                    continue;
                }
                let tentative_g = g[current] + nodes[current].distance_to(&nodes[adjacent]);
                let in_open = open.contains(&adjacent);
                if !in_open || tentative_g < g[adjacent] {
                    came_from[adjacent] = Some(current);
                    g[adjacent] = tentative_g;
                    f[adjacent] = tentative_g + nodes[adjacent].distance_to(&to);
                    if !in_open {
                        open.push(adjacent);
                    }
                }
            }
        }

        (closest != start).then(|| reconstruct(&came_from, closest))
    }

    pub fn move_relative(&self, speed: f32, relative_movement: Vector3<f64>) {
        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let movement = Self::get_relative_movement(relative_movement, speed, yaw);
        let entity = &self.mob_entity.living_entity.entity;
        let vel = entity.velocity.load();
        entity.velocity.store(vel + movement);
    }

    fn get_relative_movement(
        relative_movement: Vector3<f64>,
        speed: f32,
        yaw: f32,
    ) -> Vector3<f64> {
        let dist = relative_movement.length_squared();
        if dist < 1.0E-7 {
            return Vector3::new(0.0, 0.0, 0.0);
        }
        let vec = if dist > 1.0 {
            relative_movement.normalize()
        } else {
            relative_movement
        } * (speed as f64);
        let sin = (yaw * (std::f32::consts::PI / 180.0)).sin() as f64;
        let cos = (yaw * (std::f32::consts::PI / 180.0)).cos() as f64;
        Vector3::new(vec.x * cos - vec.z * sin, vec.y, vec.z * cos + vec.x * sin)
    }

    /// The flight part of vanilla `EnderDragon.aiStep`: steer towards the phase's target and move.
    fn fly_towards(&self, caller: &dyn EntityBase, target: Vector3<f64>, phase: &dyn phase::Phase) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let xdd = target.x - pos.x;
        let mut ydd = target.y - pos.y;
        let zdd = target.z - pos.z;
        let dist_to_target = xdd * xdd + ydd * ydd + zdd * zdd;
        let max = f64::from(phase.get_fly_speed());
        let horizontal_dist = xdd.hypot(zdd);
        if horizontal_dist > 0.0 {
            ydd = (ydd / horizontal_dist).clamp(-max, max);
        }

        let mut vel = entity.velocity.load();
        vel.y += ydd * 0.01;
        entity.velocity.store(vel);
        let yaw = wrap_degrees(entity.yaw.load());
        entity.yaw.store(yaw);

        let aim = (target - pos).normalize();
        let yaw_rad = yaw.to_radians();
        let dir =
            Vector3::new(f64::from(yaw_rad.sin()), vel.y, f64::from(-yaw_rad.cos())).normalize();
        let dot = ((dir.dot(&aim) as f32 + 0.5) / 1.5).max(0.0);
        if xdd.abs() > 1.0E-5 || zdd.abs() > 1.0E-5 {
            let y_rot_d =
                wrap_degrees(180.0 - xdd.atan2(zdd).to_degrees() as f32 - yaw).clamp(-50.0, 50.0);
            let turn_speed = phase.get_turn_speed(self);
            let y_rot_a = self.update_y_rot_a(|y_rot_a| y_rot_a * 0.8 + y_rot_d * turn_speed);
            entity.yaw.store(yaw + y_rot_a * 0.1);
        }

        let span = (2.0 / (dist_to_target + 1.0)) as f32;
        self.move_relative(
            0.06 * (dot * span + (1.0 - span)),
            Vector3::new(0.0, 0.0, -1.0),
        );
        let in_wall = *self
            .in_wall
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let vel = entity.velocity.load();
        entity.move_entity(caller, if in_wall { vel * 0.8 } else { vel });

        let actual = entity.velocity.load().normalize();
        let slide = 0.8 + 0.15 * (actual.dot(&dir) + 1.0) / 2.0;
        let vel = entity.velocity.load();
        entity
            .velocity
            .store(Vector3::new(vel.x * slide, vel.y * 0.91, vel.z * slide));
    }

    fn handle_player_collisions(&self) {
        let world = self.mob_entity.living_entity.entity.world.load();

        let dragon_bbox = self.mob_entity.living_entity.entity.bounding_box.load();
        let xm = f64::midpoint(dragon_bbox.min.x, dragon_bbox.max.x);
        let zm = f64::midpoint(dragon_bbox.min.z, dragon_bbox.max.z);

        for player in world.players.load().iter() {
            if player
                .living_entity
                .entity
                .bounding_box
                .load()
                .intersects(&dragon_bbox)
            {
                let player_pos = player.get_entity().pos.load();
                let xd = player_pos.x - xm;
                let zd = player_pos.z - zm;
                let dd = (xd * xd + zd * zd).max(0.1);

                player
                    .living_entity
                    .entity
                    .apply_knockback(4.0, xd / dd, zd / dd);
                player.get_entity().send_velocity();

                if !self.is_sitting() {
                    player.damage(self, 5.0, DamageType::MOB_ATTACK);
                }
            }
        }
    }

    /// Vanilla `EnderDragon.checkCrystals`: heal 1 every 10 ticks while a crystal is beaming at
    /// the dragon, and look for the nearest crystal now and then.
    fn check_crystals(&self) {
        let entity = self.get_entity();
        {
            let mut nearest_crystal = self
                .nearest_crystal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(crystal) = nearest_crystal.as_ref() {
                if crystal.get_entity().is_removed() {
                    *nearest_crystal = None;
                } else {
                    let living = &self.mob_entity.living_entity;
                    let health = living.health.load();
                    if entity.tick_count.load(Ordering::Relaxed) % 10 == 0
                        && health < living.get_max_health()
                    {
                        living.set_health(health + 1.0);
                    }
                }
            }
        }

        if game_random().random_range(0..10) == 0 {
            let world = entity.world.load();
            let pos = entity.pos.load();
            let search_box = entity.bounding_box.load().expand(32.0, 32.0, 32.0);
            let nearest = world
                .get_entities_at_box(&search_box)
                .into_iter()
                .filter(|crystal| crystal.get_entity().entity_type == &EntityType::END_CRYSTAL)
                .min_by(|a, b| {
                    a.get_entity()
                        .pos
                        .load()
                        .squared_distance_to_vec(&pos)
                        .total_cmp(&b.get_entity().pos.load().squared_distance_to_vec(&pos))
                });
            *self
                .nearest_crystal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = nearest;
        }
    }

    /// Vanilla `EnderDragon.onCrystalDestroyed`: losing the crystal that was healing it hurts the
    /// dragon, and the current phase may go after the player who broke it.
    pub fn on_crystal_destroyed(&self, crystal: &dyn EntityBase, cause: Option<&dyn EntityBase>) {
        let world = self.get_entity().world.load();
        let crystal_pos = crystal.get_entity().pos.load();
        let player = cause
            .and_then(|cause| world.get_player_by_id(cause.get_entity().entity_id))
            .or_else(|| {
                let block_pos = crystal.get_entity().block_pos.load();
                let targeting = TargetPredicate::create_attackable().set_base_max_distance(64.0);
                world.get_nearest_player(block_pos.0.to_f64(), f64::INFINITY, |player| {
                    targeting.test(&world, None, player.as_ref())
                })
            });

        let was_nearest = self
            .nearest_crystal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|nearest| {
                nearest.get_entity().entity_id == crystal.get_entity().entity_id
            });
        if was_nearest {
            let damage_type = if player.is_some() {
                DamageType::PLAYER_EXPLOSION
            } else {
                DamageType::EXPLOSION
            };
            self.hurt(
                self,
                true,
                10.0,
                damage_type,
                Some(crystal_pos),
                Some(crystal),
                player
                    .as_ref()
                    .map(|player| player.as_ref() as &dyn EntityBase),
            );
        }

        self.current_phase()
            .on_crystal_destroyed(self, player.as_ref());
    }

    fn tick_block_breaking(&self) {
        let world = self.mob_entity.living_entity.entity.world.load();
        let bbox = self.mob_entity.living_entity.entity.bounding_box.load();

        let min = bbox.min_block_pos();
        let max = bbox.max_block_pos();

        for pos in BlockPos::iterate(min, max) {
            let block = world.get_block(&pos);
            if block != &Block::BEDROCK
                && block != &Block::END_STONE
                && block != &Block::OBSIDIAN
                && block != &Block::IRON_BARS
                && block != &Block::END_PORTAL
                && block != &Block::END_PORTAL_FRAME
            {
                world.set_block_state(&pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
            }
        }
    }

    fn tick_parts(&self) {
        let history = self
            .flight_history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let p5 = history.get(5);
        let p10 = history.get(10);
        let p0 = history.get(0);

        let tilt = (p5.y - p10.y) as f32 * 10.0 * (std::f32::consts::PI / 180.0);
        let cc_tilt = tilt.cos() as f64;
        let ss_tilt = tilt.sin() as f64;

        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let rot1 = yaw * (std::f32::consts::PI / 180.0);
        let ss1 = rot1.sin() as f64;
        let cc1 = rot1.cos() as f64;

        let pos = self.mob_entity.living_entity.entity.pos.load();

        // Body
        self.parts[2]
            .entity
            .set_pos(Vector3::new(pos.x + ss1 * 0.5, pos.y, pos.z - cc1 * 0.5));

        // Wings
        self.parts[6].entity.set_pos(Vector3::new(
            pos.x + cc1 * 4.5,
            pos.y + 2.0,
            pos.z + ss1 * 4.5,
        ));
        self.parts[7].entity.set_pos(Vector3::new(
            pos.x - cc1 * 4.5,
            pos.y + 2.0,
            pos.z - ss1 * 4.5,
        ));

        let head_y_offset = if self.is_sitting() { -1.0 } else { p5.y - p0.y };

        let y_rot_a = *self
            .y_rot_a
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rot2 = (yaw - y_rot_a * 0.01) * (std::f32::consts::PI / 180.0);
        let ss2 = rot2.sin() as f64;
        let cc2 = rot2.cos() as f64;

        // Head & Neck
        self.parts[0].entity.set_pos(Vector3::new(
            pos.x + ss2 * 6.5 * cc_tilt,
            pos.y + head_y_offset + ss_tilt * 6.5,
            pos.z - cc2 * 6.5 * cc_tilt,
        ));
        self.parts[1].entity.set_pos(Vector3::new(
            pos.x + ss2 * 5.5 * cc_tilt,
            pos.y + head_y_offset + ss_tilt * 5.5,
            pos.z - cc2 * 5.5 * cc_tilt,
        ));

        // Tails
        for i in 0..3 {
            let pi = history.get(12 + i * 2);
            let rot = yaw * (std::f32::consts::PI / 180.0)
                + (pi.y_rot - p5.y_rot).rem_euclid(360.0).to_radians();
            let ss = rot.sin() as f64;
            let cc = rot.cos() as f64;
            let dd = (i + 1) as f64 * 2.0;

            self.parts[3 + i as usize].entity.set_pos(Vector3::new(
                pos.x - (ss1 * 1.5 + ss * dd) * cc_tilt,
                pos.y + (pi.y - p5.y) - (dd + 1.5) * ss_tilt + 1.5,
                pos.z + (cc1 * 1.5 + cc * dd) * cc_tilt,
            ));
        }
    }

    fn update_dragon_fight(&self) {
        let living = &self.mob_entity.living_entity;
        let world = living.entity.world.load();
        if let Some(fight) = &world.dragon_fight {
            let custom_name = living.entity.custom_name.load();
            fight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .update_dragon(
                    &world,
                    living.entity.entity_uuid,
                    living.health.load(),
                    living.get_max_health(),
                    custom_name.as_ref().as_ref(),
                );
        }
    }

    /// The server side of vanilla `EnderDragon.aiStep` for a living dragon.
    pub fn ai_step(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if entity.is_removed() {
            return;
        }
        entity.update_last_pos();
        let flap_time = *self
            .flap_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self
            .o_flap_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = flap_time;
        if self.mob_entity.living_entity.health.load() <= 0.0 {
            return;
        }

        self.check_crystals();
        let vel = entity.velocity.load();
        let flap_speed =
            0.2 / (vel.horizontal_length() as f32 * 10.0 + 1.0) * 2.0f64.powf(vel.y) as f32;
        let flap_increment = if self.is_sitting() {
            0.1
        } else if *self
            .in_wall
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            flap_speed * 0.5
        } else {
            flap_speed
        };
        *self
            .flap_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += flap_increment;
        entity.yaw.store(wrap_degrees(entity.yaw.load()));

        if self.mob_entity.is_no_ai() {
            *self
                .flap_time
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = 0.5;
            return;
        }

        self.flight_history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(entity.pos.load().y, entity.yaw.load());

        let phase_type = self.current_phase_type();
        self.phase_manager
            .get_phase(phase_type)
            .do_server_tick(self);
        let mut phase = self.current_phase();
        if phase.get_type() != phase_type {
            phase.do_server_tick(self);
            phase = self.current_phase();
        }
        if entity.is_removed() {
            return;
        }

        if let Some(target) = phase.get_fly_target_location() {
            self.fly_towards(caller, target, phase);
        }
        entity.body_yaw.store(entity.yaw.load());

        if !self.is_sitting() {
            self.handle_player_collisions();
            self.tick_block_breaking();
        }
        self.tick_parts();
        self.update_dragon_fight();
    }

    /// Vanilla `EnderDragon.tickDeath`: rise for ten seconds, drop experience and end the fight.
    fn tick_death(&self, caller: &dyn EntityBase) {
        self.update_dragon_fight();
        let entity = self.get_entity();
        let world = entity.world.load();
        let death_time = {
            let mut death_time = self
                .dragon_death_time
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *death_time += 1;
            *death_time
        };

        let xp_count = if self
            .with_dragon_fight(|fight| fight.has_previously_killed_dragon())
            .is_some_and(|killed| !killed)
        {
            12000
        } else {
            500
        };
        let mob_drops = world.level_info.load().game_rules.mob_drops;
        if death_time > 150 && death_time % 5 == 0 && mob_drops {
            ExperienceOrbEntity::spawn(&world, entity.pos.load(), (xp_count as f32 * 0.08) as u32);
        }
        if death_time == 1 && !entity.is_silent() {
            world.sync_global_world_event(
                WorldEvent::SoundDragonDeath,
                BlockPos::floored_v(entity.pos.load()),
                0,
            );
        }

        let death_move = Vector3::new(0.0, f64::from(0.1f32), 0.0);
        entity.move_entity(caller, death_move);
        for part in &self.parts {
            part.entity.set_pos(part.entity.pos.load() + death_move);
        }

        if death_time >= DEATH_TIME_MAX {
            if mob_drops {
                ExperienceOrbEntity::spawn(
                    &world,
                    entity.pos.load(),
                    (xp_count as f32 * 0.2) as u32,
                );
            }
            if let Some(fight) = &world.dragon_fight {
                fight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_dragon_killed(&world, entity.entity_uuid);
            }
            for part in &self.parts {
                part.entity.remove();
            }
            entity.remove();
            entity.emit_own_game_event(caller, GameEvent::EntityDie);
        }
    }

    /// Vanilla `EnderDragon.hurt(ServerLevel, EnderDragonPart, DamageSource, float)`. `caller` is
    /// the dragon itself. Only players and `#always_hurts_ender_dragons` damage it, but other
    /// hits still count as landed.
    #[expect(clippy::too_many_arguments)]
    pub fn hurt(
        &self,
        caller: &dyn EntityBase,
        head_or_neck: bool,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let phase = self.current_phase();
        if phase.get_type() == EnderDragonPhase::Dying {
            return false;
        }
        let mut damage = phase.on_hurt(source, amount);
        if !head_or_neck {
            damage = damage / 4.0 + damage.min(1.0);
        }
        if damage < 0.01 {
            return false;
        }

        if cause.is_some_and(|cause| cause.get_player().is_some())
            || damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_HURTS_ENDER_DRAGONS)
        {
            let living = &self.mob_entity.living_entity;
            let health_before = living.health.load();
            // Vanilla `reallyHurt`: the plain living entity damage.
            if living.damage_with_context(caller, damage, damage_type, position, source, cause) {
                self.on_damage_with_context(damage_type, source, cause);
            }
            if self.is_sitting() {
                let mut received = self
                    .sitting_damage_received
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *received += health_before - living.health.load();
                if *received > SITTING_ALLOWED_DAMAGE_PERCENTAGE * living.get_max_health() {
                    *received = 0.0;
                    drop(received);
                    self.set_phase(EnderDragonPhase::Takeoff);
                }
            }
        }
        true
    }
}

impl Mob for EnderDragonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        // Vanilla `LivingEntity.baseTick` runs `tickDeath` before `aiStep`.
        if self.mob_entity.living_entity.health.load() <= 0.0 && !self.get_entity().is_removed() {
            self.tick_death(caller);
        }
        self.ai_step(caller);
    }

    /// The dragon moves itself in `aiStep` instead of travelling like other mobs.
    fn travel(&self, _caller: &dyn EntityBase) -> bool {
        true
    }

    fn mob_init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            tracked_data::ender_dragon::DATA_PHASE,
            VarInt(self.current_phase_type().id()),
        );
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("DragonPhase", self.current_phase_type().id());
        nbt.put_int(
            "DragonDeathTime",
            *self
                .dragon_death_time
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_float(
            "sitting_damage_received",
            *self
                .sitting_damage_received
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(phase) = nbt.get_int("DragonPhase") {
            self.set_phase(EnderDragonPhase::from_id(phase));
        }
        *self
            .dragon_death_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            nbt.get_int("DragonDeathTime").unwrap_or(0);
        *self
            .sitting_damage_received
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            nbt.get_float("sitting_damage_received").unwrap_or(0.0);
    }

    /// Vanilla `EnderDragon.hurtServer`: a hit on the dragon itself counts as one on its body.
    fn mob_hurt_server(
        &self,
        caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> Option<bool> {
        Some(self.hurt(caller, false, amount, damage_type, position, source, cause))
    }

    /// Vanilla `EnderDragon.tickDeath`: the dragon runs its own 200 tick death.
    fn ticks_own_death(&self) -> bool {
        true
    }

    /// Vanilla `EnderDragon.kill`: gone at once, without the death animation or experience.
    fn mob_kill(&self, caller: &dyn EntityBase) -> bool {
        let entity = self.get_entity();
        for part in &self.parts {
            part.entity.remove();
        }
        entity.remove();
        entity.emit_own_game_event(caller, GameEvent::EntityDie);
        let world = entity.world.load_full();
        if let Some(fight) = &world.dragon_fight {
            fight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_dragon_killed(&world, entity.entity_uuid);
        }
        true
    }

    /// Vanilla `EnderDragon.handleKillingBlow`: a flying dragon is kept at 1 health and flies
    /// back to the portal in the dying phase; a sitting one dies where it is.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        if living.health.load() <= 0.0 && !self.is_sitting() {
            living.set_health(1.0);
            self.set_phase(EnderDragonPhase::Dying);
        }
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn get_owned_entities(&self) -> Vec<Arc<dyn EntityBase>> {
        self.parts
            .iter()
            .map(|part| part.clone() as Arc<dyn EntityBase>)
            .collect()
    }
}
