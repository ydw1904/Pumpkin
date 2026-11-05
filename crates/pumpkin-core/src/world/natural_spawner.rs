use crate::block::PathComputationType;
use crate::entity::EntityBase;
use crate::entity::mob::spawn::{PendingVehicle, finalize_spawn};
use crate::entity::player::Player;
use crate::entity::r#type::{
    CreatureSpawnLevel, check_creature_spawn_rules, check_spawn_rules, from_type,
};
use crate::world::World;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use pumpkin_data::biome::{SpawnCosts, Spawner};
use pumpkin_data::chunk::Biome;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::{EntityType, MobCategory, SpawnLocation};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::structures::{BoundingBoxType, Structure, StructureKeys};
use pumpkin_data::tag::Block::MINECRAFT_PREVENT_MOB_SPAWNING_INSIDE;
use pumpkin_data::tag::Fluid::{MINECRAFT_LAVA, MINECRAFT_WATER};
use pumpkin_data::tag::Taggable;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_REDUCE_WATER_AMBIENT_SPAWNS;
use pumpkin_data::{Block, BlockDirection, BlockState, SpawnFloorPredicate};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::get_section_cord;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::chunk::{ChunkData, ChunkHeightmapType};
use pumpkin_world::generation::proto_chunk::GenerationCache;
use rustc_hash::FxHashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use uuid::Uuid;

const MAGIC_NUMBER: i32 = 17 * 17;
/// The cost of a mob its biome sets none for: no charge, so it always fits the spawn potential.
static NO_SPAWN_COST: SpawnCosts = SpawnCosts {
    energy_budget: f64::INFINITY,
    charge: 0.0,
};

const MOB_CATEGORY_COUNT: usize = 8;

pub struct MobCounts([AtomicI32; MOB_CATEGORY_COUNT]);

impl Default for MobCounts {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicI32::new(0)))
    }
}

impl fmt::Debug for MobCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|a| a.load(Relaxed)))
            .finish()
    }
}

impl Clone for MobCounts {
    fn clone(&self) -> Self {
        Self(std::array::from_fn(|i| {
            AtomicI32::new(self.0[i].load(Relaxed))
        }))
    }
}

impl MobCounts {
    #[inline]
    pub fn add(&self, category: &'static MobCategory) {
        self.0[category.id].fetch_add(1, Relaxed);
    }

    fn add_counts(&self, counts: &[i32; MOB_CATEGORY_COUNT]) {
        for (count, &added) in self.0.iter().zip(counts) {
            count.fetch_add(added, Relaxed);
        }
    }

    #[inline]
    pub fn can_spawn(&self, category: &'static MobCategory) -> bool {
        self.0[category.id].load(Relaxed) < category.max
    }
}

pub struct LocalMobCapCalculator {
    player_mob_counts: DashMap<i32, MobCounts>,
    players_near_chunk: DashMap<Vector2<i32>, Arc<[i32]>>,
}

impl Clone for LocalMobCapCalculator {
    fn clone(&self) -> Self {
        let player_mob_counts = DashMap::new();
        for r in &self.player_mob_counts {
            player_mob_counts.insert(*r.key(), r.value().clone());
        }
        let players_near_chunk = DashMap::new();
        for r in &self.players_near_chunk {
            players_near_chunk.insert(*r.key(), r.value().clone());
        }
        Self {
            player_mob_counts,
            players_near_chunk,
        }
    }
}

impl Default for LocalMobCapCalculator {
    fn default() -> Self {
        Self {
            player_mob_counts: DashMap::new(),
            players_near_chunk: DashMap::new(),
        }
    }
}

impl fmt::Debug for LocalMobCapCalculator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalMobCapCalculator")
            .field("world", &"<skipped>")
            .finish()
    }
}

/// Vanilla `ChunkMap.euclideanDistanceSquared`: horizontal distance from the chunk's center.
const fn chunk_center_distance_squared(chunk_pos: Vector2<i32>, player_pos: &Vector3<f64>) -> f64 {
    let dx = ((chunk_pos.x << 4) + 8) as f64 - player_pos.x;
    let dy = ((chunk_pos.y << 4) + 8) as f64 - player_pos.z;
    dx * dx + dy * dy
}

/// Vanilla `ChunkMap.playerIsCloseEnoughForSpawning`: a non-spectator within 128 blocks of the
/// chunk's center.
fn player_is_close_enough_for_spawning(player: &Player, chunk_pos: Vector2<i32>) -> bool {
    player.gamemode.load() != GameMode::Spectator
        && chunk_center_distance_squared(chunk_pos, &player.position()) < 16384.0
}

/// Vanilla `ChunkMap.anyPlayerCloseEnoughForSpawningInternal`.
pub fn any_player_close_enough_for_spawning(
    players: &[Arc<Player>],
    chunk_pos: Vector2<i32>,
) -> bool {
    players
        .iter()
        .any(|player| player_is_close_enough_for_spawning(player, chunk_pos))
}

impl LocalMobCapCalculator {
    fn get_players_near(&self, world: &World, chunk_pos: Vector2<i32>) -> Arc<[i32]> {
        if let Some(players) = self.players_near_chunk.get(&chunk_pos) {
            return players.value().clone();
        }

        let mut players = Vec::new();
        for player in world.players.load().iter() {
            if player_is_close_enough_for_spawning(player, chunk_pos) {
                players.push(player.entity_id());
            }
        }
        let players: Arc<[i32]> = players.into();
        self.players_near_chunk.insert(chunk_pos, players.clone());
        players
    }

    pub fn add_mob(&self, chunk_pos: Vector2<i32>, world: &World, category: &'static MobCategory) {
        let players = self.get_players_near(world, chunk_pos);
        for &player in players.iter() {
            self.player_mob_counts
                .entry(player)
                .or_default()
                .add(category);
        }
    }

    /// Adds a chunk's mobs counted by category, like [`Self::add_mob`] for each of them.
    fn add_mobs(&self, chunk_pos: Vector2<i32>, world: &World, counts: &[i32; MOB_CATEGORY_COUNT]) {
        let players = self.get_players_near(world, chunk_pos);
        for &player in players.iter() {
            self.player_mob_counts
                .entry(player)
                .or_default()
                .add_counts(counts);
        }
    }

    pub fn can_spawn(
        &self,
        category: &'static MobCategory,
        world: &World,
        chunk_pos: Vector2<i32>,
    ) -> bool {
        let players = self.get_players_near(world, chunk_pos);
        for &player in players.iter() {
            if let Some(count) = self.player_mob_counts.get(&player) {
                if count.can_spawn(category) {
                    return true;
                }
            } else {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct PointCharge(BlockPos, f64);

impl PointCharge {
    /// Change in spawn potential this charge adds at `pos`.
    ///
    /// Widen before subtracting, like vanilla's `Vec3i.distSqr` does with `i2d`;
    /// multiplying in `i32` overflows past ~46341 blocks before any cast runs.
    fn get_potential_change(&self, pos: &BlockPos) -> f64 {
        let dx = self.0.0.x as f64 - pos.0.x as f64;
        let dy = self.0.0.y as f64 - pos.0.y as f64;
        let dz = self.0.0.z as f64 - pos.0.z as f64;
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq == 0.0 {
            f64::INFINITY
        } else {
            self.1 / dist_sq.sqrt()
        }
    }
}

#[derive(Default, Debug)]
struct PotentialCalculator(std::sync::Mutex<Vec<PointCharge>>);

impl Clone for PotentialCalculator {
    fn clone(&self) -> Self {
        Self(std::sync::Mutex::new(
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        ))
    }
}

impl PotentialCalculator {
    pub fn add_charge(&self, pos: &BlockPos, charge: f64) {
        if charge != 0.0 {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(PointCharge(*pos, charge));
        }
    }

    /// Vanilla `PotentialCalculator.getPotentialEnergyChange`, with `extra` charges added after
    /// this calculator's own.
    pub fn get_potential_energy_change(
        &self,
        pos: &BlockPos,
        charge: f64,
        extra: &[PointCharge],
    ) -> f64 {
        if charge == 0.0 {
            return 0.0;
        }
        let mut sum: f64 = 0.0;
        let charges = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for i in charges.iter().chain(extra) {
            sum += i.get_potential_change(pos);
        }
        sum * charge
    }
}

/// A mob a chunk decided to spawn, waiting for [`SpawnState::commit`].
struct PendingSpawn {
    entity: Arc<dyn EntityBase>,
    pos: BlockPos,
    cost: &'static SpawnCosts,
}

/// The mobs one chunk decided to spawn. Chunks decide in parallel against the counts the tick
/// started with, and [`SpawnState::commit`] spawns them in vanilla's chunk order.
#[derive(Default)]
pub struct ChunkSpawns {
    /// The charges of `spawns`, which this chunk's later spawn checks include.
    charges: Vec<PointCharge>,
    spawns: Vec<PendingSpawn>,
}

pub struct SpawnState {
    spawnable_chunk_count: i32,
    pub mob_category_counts: MobCounts,
    spawn_potential: PotentialCalculator,
    local_mob_cap_calculator: LocalMobCapCalculator,
}

impl Clone for SpawnState {
    fn clone(&self) -> Self {
        Self {
            spawnable_chunk_count: self.spawnable_chunk_count,
            mob_category_counts: self.mob_category_counts.clone(),
            spawn_potential: self.spawn_potential.clone(),
            local_mob_cap_calculator: self.local_mob_cap_calculator.clone(),
        }
    }
}

impl fmt::Debug for SpawnState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnState")
            .field("spawnable_chunk_count", &self.spawnable_chunk_count)
            .field("mob_category_counts", &self.mob_category_counts)
            .field("spawn_potential", &self.spawn_potential)
            .field("local_mob_cap_calculator", &self.local_mob_cap_calculator)
            .finish()
    }
}

impl SpawnState {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            spawnable_chunk_count: 0,
            mob_category_counts: MobCounts::default(),
            spawn_potential: PotentialCalculator::default(),
            local_mob_cap_calculator: LocalMobCapCalculator::default(),
        }
    }

    pub const fn set_spawnable_chunk_count(&mut self, count: i32) {
        self.spawnable_chunk_count = count;
    }

    pub fn new(
        chunk_count: i32,
        entities: &ArcSwap<Vec<Arc<dyn EntityBase>>>,
        world: &World,
    ) -> Self {
        let mut charges = Vec::new();
        let mut category_counts = [0; MOB_CATEGORY_COUNT];
        // Mobs are counted per chunk first, so each chunk's nearby players are looked up once.
        let mut chunk_mob_counts: FxHashMap<Vector2<i32>, [i32; MOB_CATEGORY_COUNT]> =
            FxHashMap::default();
        let mut loaded_chunks: FxHashMap<Vector2<i32>, bool> = FxHashMap::default();
        for entity in entities.load().iter() {
            let base_entity = entity.get_entity();
            let entity_type = base_entity.entity_type;
            if entity_type.category == &MobCategory::MISC {
                continue;
            }
            let mob = entity.get_mob();
            if let Some(mob) = mob
                && (mob.get_mob_entity().persistence_required.load(Relaxed)
                    || mob.requires_custom_persistence())
            {
                continue;
            }
            // Vanilla `NaturalSpawner.createState` counts mobs in every loaded full chunk.
            let chunk_pos = base_entity.chunk_pos.load();
            if !*loaded_chunks
                .entry(chunk_pos)
                .or_insert_with(|| world.level.is_chunk_loaded(&chunk_pos))
            {
                continue;
            }
            let biome = base_entity.current_biome.load();
            if let Some(cost) = biome.spawn_costs.get(entity_type.resource_name)
                && cost.charge != 0.0
            {
                charges.push(PointCharge(base_entity.block_pos.load(), cost.charge));
            }
            if mob.is_some() || entity_type.mob {
                chunk_mob_counts.entry(chunk_pos).or_default()[entity_type.category.id] += 1;
            }
            category_counts[entity_type.category.id] += 1;
        }
        let local_mob_cap = LocalMobCapCalculator::default();
        for (chunk_pos, counts) in chunk_mob_counts {
            local_mob_cap.add_mobs(chunk_pos, world, &counts);
        }
        Self {
            spawnable_chunk_count: chunk_count,
            mob_category_counts: MobCounts(category_counts.map(AtomicI32::new)),
            spawn_potential: PotentialCalculator(std::sync::Mutex::new(charges)),
            local_mob_cap_calculator: local_mob_cap,
        }
    }

    #[inline]
    pub fn can_spawn_for_category_global(&self, category: &'static MobCategory) -> bool {
        self.mob_category_counts.0[category.id].load(Relaxed)
            < category.max * self.spawnable_chunk_count / MAGIC_NUMBER
    }

    pub fn can_spawn_for_category_local(
        &self,
        world: &Arc<World>,
        category: &'static MobCategory,
        chunk_pos: Vector2<i32>,
    ) -> bool {
        self.local_mob_cap_calculator
            .can_spawn(category, world, chunk_pos)
    }

    /// Vanilla `SpawnState.canSpawn`, which also counts the mobs `chunk` decided on so far.
    /// Returns the biome's spawn cost for the mob, which [`Self::commit`] checks again.
    fn can_spawn(
        &self,
        entity_type: &'static EntityType,
        pos: &BlockPos,
        world: &Arc<World>,
        chunk: &ChunkSpawns,
    ) -> Option<&'static SpawnCosts> {
        let cost = world
            .level
            .get_rough_biome(pos)
            .spawn_costs
            .get(entity_type.resource_name)
            .unwrap_or(&NO_SPAWN_COST);
        (self
            .spawn_potential
            .get_potential_energy_change(pos, cost.charge, &chunk.charges)
            <= cost.energy_budget)
            .then_some(cost)
    }

    /// Vanilla `SpawnState.afterSpawn`.
    fn after_spawn(
        &self,
        entity_type: &'static EntityType,
        pos: &BlockPos,
        charge: f64,
        world: &Arc<World>,
    ) {
        self.spawn_potential.add_charge(pos, charge);
        self.mob_category_counts.add(entity_type.category);
        self.local_mob_cap_calculator.add_mob(
            Vector2::<i32>::new(get_section_cord(pos.0.x), get_section_cord(pos.0.z)),
            world,
            entity_type.category,
        );
    }

    /// Spawns the mobs `chunk` decided on as vanilla's sequential `spawnForChunk` would after
    /// the chunks before it: each category passes the local mob cap again and each mob its
    /// spawn potential, now counting what those chunks spawned.
    pub fn commit(&self, world: &Arc<World>, chunk_pos: Vector2<i32>, chunk: ChunkSpawns) {
        let mut category_allowed: Option<(&'static MobCategory, bool)> = None;
        for spawn in chunk.spawns {
            let entity_type = spawn.entity.get_entity().entity_type;
            let category = entity_type.category;
            let allowed = match category_allowed {
                Some((checked, allowed)) if checked.id == category.id => allowed,
                _ => {
                    let allowed = self.can_spawn_for_category_local(world, category, chunk_pos);
                    category_allowed = Some((category, allowed));
                    allowed
                }
            };
            if !allowed
                || self.spawn_potential.get_potential_energy_change(
                    &spawn.pos,
                    spawn.cost.charge,
                    &[],
                ) > spawn.cost.energy_budget
            {
                continue;
            }
            world.spawn_entity_non_save(spawn.entity);
            self.after_spawn(entity_type, &spawn.pos, spawn.cost.charge, world);
        }
    }
}

#[must_use]
pub fn get_filtered_spawning_categories(
    state: &SpawnState,
    spawn_friendlies: bool,
    spawn_enemies: bool,
    spawn_passives: bool,
) -> Vec<&'static MobCategory> {
    let mut ret = Vec::with_capacity(MobCategory::SPAWNING_CATEGORIES.len());
    for category in MobCategory::SPAWNING_CATEGORIES {
        let is_type_allowed = if category.is_friendly {
            spawn_friendlies
        } else {
            spawn_enemies
        };

        if !is_type_allowed {
            continue;
        }

        if category.is_persistent && !spawn_passives {
            continue;
        }

        if state.can_spawn_for_category_global(category) {
            ret.push(category);
        }
    }
    ret
}

pub fn spawn_for_chunk(
    world: &Arc<World>,
    chunk_pos: Vector2<i32>,
    chunk: &Arc<ChunkData>,
    spawn_state: &SpawnState,
    spawn_list: &Vec<&'static MobCategory>,
    is_thundering: bool,
) -> ChunkSpawns {
    let mut spawns = ChunkSpawns::default();
    for category in spawn_list {
        // The counts only grow until `commit`, so a category that fails here fails there too.
        if spawn_state.can_spawn_for_category_local(world, category, chunk_pos) {
            let random_pos = get_random_pos_within(world.min_y, &chunk_pos, chunk);
            if random_pos.0.y > world.min_y {
                spawn_category_for_position(
                    category,
                    world,
                    random_pos,
                    &chunk_pos,
                    spawn_state,
                    &mut spawns,
                    is_thundering,
                );
            }
        }
    }
    spawns
}

pub fn get_random_pos_within(
    min_y: i32,
    chunk_pos: &Vector2<i32>,
    chunk: &Arc<ChunkData>,
) -> BlockPos {
    let rng = game_random();

    let x = (chunk_pos.x << 4) + rng.next_bounded_i32(16);
    let z = (chunk_pos.y << 4) + rng.next_bounded_i32(16);
    let temp_y = chunk
        .heightmap
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(ChunkHeightmapType::WorldSurface, x, z, chunk.section.min_y)
        + 1;
    let y = rng.next_inbetween_i32(min_y, temp_y);
    BlockPos::new(x, y, z)
}

pub fn spawn_mobs_for_chunk_generation(
    world: &Arc<World>,
    cache: &mut dyn GenerationCache,
    biome: &'static Biome,
    chunk_x: i32,
    chunk_z: i32,
) {
    let mob_settings = &biome.spawners;
    let creatures = &mob_settings.creature;

    if creatures.is_empty() || !world.level_info.load().game_rules.spawn_mobs {
        return;
    }

    let xo = chunk_x << 4;
    let zo = chunk_z << 4;

    while game_random().random::<f32>() < biome.creature_spawn_probability {
        let Some(spawner_data) = choose_spawner(creatures) else {
            continue;
        };

        let count = spawner_data.min_count
            + game_random()
                .random_range(0..=(spawner_data.max_count - spawner_data.min_count).max(0));
        let name = spawner_data
            .r#type
            .strip_prefix("minecraft:")
            .unwrap_or(spawner_data.r#type);
        let Some(entity_type) = EntityType::from_name(name) else {
            return;
        };

        let mut x = xo + game_random().random_range(0..16);
        let mut z = zo + game_random().random_range(0..16);
        let start_x = x;
        let start_z = z;

        let mut group_data = None;
        for _ in 0..count {
            let mut success = false;

            for _ in 0..4 {
                if success {
                    break;
                }

                let pos = get_top_non_colliding_pos(world, cache, entity_type, x, z);

                if entity_type.summonable && is_spawn_position_ok_cache(cache, &pos, entity_type) {
                    let width = f64::from(entity_type.dimension[0]);
                    let fx =
                        (f64::from(x)).clamp(f64::from(xo) + width, f64::from(xo) + 16.0 - width);
                    let fz =
                        (f64::from(z)).clamp(f64::from(zo) + width, f64::from(zo) + 16.0 - width);
                    let spawn_pos_f64 = Vector3::new(fx, f64::from(pos.0.y), fz);
                    let check_pos = BlockPos::new(fx.floor() as i32, pos.0.y, fz.floor() as i32);

                    let level = GenerationSpawnLevel {
                        cache,
                        ambient_light: world.dimension.ambient_light,
                        fixed_light: world.level.lighting_config.fixed_level(),
                    };
                    if !level.is_space_empty(entity_type.get_spawn_bounding_box(
                        fx,
                        f64::from(pos.0.y),
                        fz,
                    )) || !check_creature_spawn_rules(
                        entity_type,
                        &level,
                        &check_pos,
                        crate::entity::mob::spawn::EntitySpawnReason::ChunkGeneration,
                    )
                    .unwrap_or_else(|| {
                        check_spawn_rules(
                            entity_type,
                            world,
                            &check_pos,
                            false,
                            crate::entity::mob::spawn::EntitySpawnReason::ChunkGeneration,
                        )
                    }) {
                        // Vanilla `continue`s here, so the next attempt retries the same column.
                        continue;
                    }
                    let entity = from_type(entity_type, spawn_pos_f64, world, Uuid::new_v4());
                    entity
                        .get_entity()
                        .set_rotation(game_random().random::<f32>() * 360.0, 0.0);
                    // Vanilla `Mob.checkSpawnRules` is a walk target check.
                    if entity.get_mob().is_some_and(|mob| {
                        mob.get_walk_target_value(&level, &check_pos) >= 0.0
                            && mob.check_spawn_obstruction(&level)
                    }) {
                        group_data = finalize_spawn(
                            &entity,
                            world,
                            crate::entity::mob::spawn::EntitySpawnReason::ChunkGeneration,
                            group_data,
                        );
                        cache
                            .get_center_chunk_mut()
                            .add_entity(world_gen_entity_nbt(&entity));
                        success = true;
                    }
                }

                x += game_random().random_range(0..5) - game_random().random_range(0..5);
                z += game_random().random_range(0..5) - game_random().random_range(0..5);
                while x < xo || x >= xo + 16 || z < zo || z >= zo + 16 {
                    x = start_x + game_random().random_range(0..5)
                        - game_random().random_range(0..5);
                    z = start_z + game_random().random_range(0..5)
                        - game_random().random_range(0..5);
                }
            }
        }
    }
}

/// The saved data a generated chunk keeps for an entity generation spawned.
///
/// Vanilla adds it with `addFreshEntityWithPassengers` to a `ProtoChunk`, which stores the root
/// entity with its passengers, so the riders or new vehicle the spawn finalization made are
/// mounted first.
pub fn world_gen_entity_nbt(entity: &Arc<dyn EntityBase>) -> NbtCompound {
    let mut root = entity.clone();
    if let Some(mob) = entity.get_mob() {
        let mob_entity = mob.get_mob_entity();
        for rider in mob_entity.take_pending_riders() {
            entity
                .get_entity()
                .add_passenger_silently(entity.clone(), rider);
        }
        // A vehicle already in the world can't be part of a generated chunk; vanilla's
        // generation region has no entities to find one in.
        if let Some(PendingVehicle::New(vehicle)) = mob_entity.take_pending_vehicle() {
            vehicle
                .get_entity()
                .add_passenger_silently(vehicle.clone(), entity.clone());
            root = vehicle;
        }
    }
    let mut nbt = NbtCompound::new();
    root.write_nbt(&mut nbt);
    nbt
}

/// The region chunk generation spawns animals in. Its chunks aren't part of the world yet, so
/// the spawn rules read them through the generation cache. Its entities are only kept as saved
/// data, so none of them can be in the way.
struct GenerationSpawnLevel<'a> {
    cache: &'a dyn GenerationCache,
    ambient_light: f32,
    /// The light level of a full or dark lighting mode, which never lights generating chunks.
    fixed_light: Option<u8>,
}

impl CreatureSpawnLevel for GenerationSpawnLevel<'_> {
    fn block_state(&self, pos: &BlockPos) -> &'static BlockState {
        GenerationCache::get_block_state(self.cache, &pos.0).to_state()
    }

    fn block(&self, pos: &BlockPos) -> &'static Block {
        Block::from_state_id(GenerationCache::get_block_state(self.cache, &pos.0))
    }

    fn fluid(&self, pos: &BlockPos) -> &'static Fluid {
        World::get_fluid_from_state_id(GenerationCache::get_block_state(self.cache, &pos.0))
    }

    fn pathfinding_cost_from_light_levels(&self, pos: &BlockPos) -> f32 {
        // Vanilla `WorldGenRegion.getSkyDarken` is 0, so the local brightness is the raw one.
        super::brightness::light_level_curve(self.raw_brightness(pos), self.ambient_light) - 0.5
    }

    fn is_space_empty(&self, bounding_box: BoundingBox) -> bool {
        BlockPos::iterate(bounding_box.min_block_pos(), bounding_box.max_block_pos()).all(|pos| {
            !World::check_collision(&bounding_box, pos, self.block_state(&pos), false, |_| ())
        })
    }

    fn entities_at_box(&self, _bounding_box: &BoundingBox) -> Vec<Arc<dyn EntityBase>> {
        Vec::new()
    }

    fn players_at_box(&self, _bounding_box: &BoundingBox) -> Vec<Arc<Player>> {
        Vec::new()
    }

    fn raw_brightness(&self, pos: &BlockPos) -> u8 {
        if let Some(level) = self.fixed_light {
            return level;
        }
        let Some(chunk) = self.cache.get_chunk(pos.0.x >> 4, pos.0.z >> 4) else {
            return 0;
        };
        let Ok(section) = usize::try_from((pos.0.y - i32::from(chunk.bottom_y())) >> 4) else {
            return 0;
        };
        let (x, y, z) = (
            (pos.0.x & 15) as usize,
            (pos.0.y & 15) as usize,
            (pos.0.z & 15) as usize,
        );
        // Above the top section the sky is fully lit.
        let sky = chunk
            .light
            .sky_light
            .get(section)
            .map_or(15, |light| light.get(x, y, z));
        let block = chunk
            .light
            .block_light
            .get(section)
            .map_or(0, |light| light.get(x, y, z));
        sky.max(block)
    }

    fn biome(&self, pos: &BlockPos) -> &'static Biome {
        self.cache
            .get_biome_for_terrain_gen(pos.0.x, pos.0.y, pos.0.z)
    }

    fn sea_level(&self) -> i32 {
        self.cache.get_sea_level()
    }
}

pub fn get_top_non_colliding_pos(
    world: &World,
    cache: &dyn GenerationCache,
    entity_type: &'static EntityType,
    x: i32,
    z: i32,
) -> BlockPos {
    let mut y = cache.get_top_y(&entity_type.spawn_restriction.heightmap, x, z);
    let mut pos_vec = Vector3::new(x, y, z);
    let min_y = world.min_y;

    if world.dimension.has_ceiling {
        loop {
            y -= 1;
            pos_vec.y = y;
            if GenerationCache::get_block_state(cache, &pos_vec)
                .to_state()
                .is_air()
                || y <= min_y
            {
                break;
            }
        }

        loop {
            y -= 1;
            pos_vec.y = y;
            if !GenerationCache::get_block_state(cache, &pos_vec)
                .to_state()
                .is_air()
                || y <= min_y
            {
                break;
            }
        }
    }

    let pos = BlockPos::new(x, y, z);

    adjust_spawn_position_cache(cache, pos, entity_type)
}

pub fn adjust_spawn_position_cache(
    cache: &dyn GenerationCache,
    pos: BlockPos,
    entity_type: &'static EntityType,
) -> BlockPos {
    if matches!(
        entity_type.spawn_restriction.location,
        SpawnLocation::OnGround
    ) {
        let below = pos.down();
        let state = GenerationCache::get_block_state(cache, &below.0).to_state();

        if !state.is_full_cube() && !state.is_liquid() {
            return below;
        }
    }
    pos
}

pub fn adjust_spawn_position(
    world: &World,
    pos: BlockPos,
    entity_type: &'static EntityType,
) -> BlockPos {
    if matches!(
        entity_type.spawn_restriction.location,
        SpawnLocation::OnGround
    ) {
        let below = pos.down();
        let (block, state) = world.get_block_and_state(&below);
        if world
            .block_registry
            .is_pathfindable(block, state, PathComputationType::Land)
        {
            return below;
        }
    }
    pos
}

#[allow(clippy::too_many_lines)]
pub fn spawn_category_for_position(
    category: &'static MobCategory,
    world: &Arc<World>,
    pos: BlockPos,
    chunk_pos: &Vector2<i32>,
    spawn_state: &SpawnState,
    spawns: &mut ChunkSpawns,
    is_thundering: bool,
) {
    let y_start = pos.0.y;
    let state = world.get_block_state(&pos);
    if state.is_solid_block() || state.is_full_cube() {
        return;
    }

    let mut cluster_size = 0;

    for _ in 0..3 {
        let mut x = pos.0.x;
        let mut z = pos.0.z;
        let mut current_spawner: Option<&'static Spawner> = None;
        let mut max = (game_random().random::<f32>() * 4.0).ceil() as i32;
        let mut group_size = 0;
        let mut group_data = None;
        let mut ll = 0;

        while ll < max {
            x += game_random().random_range(0..6) - game_random().random_range(0..6);
            z += game_random().random_range(0..6) - game_random().random_range(0..6);
            let check_pos = BlockPos::new(x, y_start, z);
            let xx = f64::from(x) + 0.5;
            let zz = f64::from(z) + 0.5;

            let nearest_player_distance_sq = {
                let players = world.players.load();
                let mut min_dst_sq = f64::MAX;
                let mut found = false;
                for player in players.iter() {
                    if player.gamemode.load() != GameMode::Spectator {
                        let ppos = player.position();
                        let dx = xx - ppos.x;
                        let dy = f64::from(y_start) - ppos.y;
                        let dz = zz - ppos.z;
                        let dist_sq = dx * dx + dy * dy + dz * dz;
                        if dist_sq < min_dst_sq {
                            min_dst_sq = dist_sq;
                            found = true;
                        }
                    }
                }
                found.then_some(min_dst_sq)
            };

            if let Some(player_distance_sq) = nearest_player_distance_sq
                && is_right_distance_to_player_and_spawn_point(
                    world,
                    &check_pos,
                    chunk_pos,
                    player_distance_sq,
                )
            {
                if current_spawner.is_none() {
                    let Some(spawner) = get_random_spawn_mob_at(world, category, &check_pos) else {
                        break;
                    };
                    current_spawner = Some(spawner);
                    max = spawner.min_count
                        + game_random()
                            .random_range(0..=(spawner.max_count - spawner.min_count).max(0));
                }

                let Some(spawner) = current_spawner else {
                    break;
                };
                let name = spawner
                    .r#type
                    .strip_prefix("minecraft:")
                    .unwrap_or(spawner.r#type);
                let Some(entity_type) = EntityType::from_name(name) else {
                    break;
                };

                if is_valid_spawn_position_for_type(
                    world,
                    &check_pos,
                    category,
                    entity_type,
                    spawner,
                    player_distance_sq,
                    is_thundering,
                ) && let Some(cost) =
                    spawn_state.can_spawn(entity_type, &check_pos, world, spawns)
                {
                    let spawn_pos_f64 = Vector3::new(xx, f64::from(y_start), zz);
                    let entity = from_type(entity_type, spawn_pos_f64, world, Uuid::new_v4());
                    entity
                        .get_entity()
                        .set_rotation(game_random().random::<f32>() * 360.0, 0.0);

                    // Vanilla `isValidPositionForMob`; `Mob.checkSpawnRules` is a walk target check.
                    let is_valid_for_mob = entity.get_mob().is_none_or(|mob| {
                        let despawn_dist = f64::from(entity_type.category.despawn_distance);
                        !(player_distance_sq > despawn_dist * despawn_dist
                            && mob.remove_when_far_away(player_distance_sq))
                            && mob.get_walk_target_value(world.as_ref(), &check_pos) >= 0.0
                            && mob.check_spawn_obstruction(world.as_ref())
                    });

                    if is_valid_for_mob {
                        group_data = finalize_spawn(
                            &entity,
                            world,
                            crate::entity::mob::spawn::EntitySpawnReason::Natural,
                            group_data,
                        );
                        cluster_size += 1;
                        group_size += 1;
                        let max_group_size_reached = entity
                            .get_mob()
                            .is_some_and(|mob| mob.is_max_group_size_reached(group_size));
                        if cost.charge != 0.0 {
                            spawns.charges.push(PointCharge(check_pos, cost.charge));
                        }
                        spawns.spawns.push(PendingSpawn {
                            entity,
                            pos: check_pos,
                            cost,
                        });
                        if cluster_size >= entity_type.limit_per_chunk {
                            return;
                        }

                        if max_group_size_reached {
                            break;
                        }
                    }
                }
            }

            ll += 1;
        }
    }
}

#[must_use]
pub fn get_nearest_player(pos: &Vector3<f64>, player_positions: &[Vector3<f64>]) -> f64 {
    let mut min_dst_sq = f64::MAX;

    for player_pos in player_positions {
        let cur_dst_sq = player_pos.squared_distance_to_vec(pos);
        if cur_dst_sq < min_dst_sq {
            min_dst_sq = cur_dst_sq;
        }
    }
    min_dst_sq
}

#[must_use]
pub fn is_right_distance_to_player_and_spawn_point(
    world: &World,
    pos: &BlockPos,
    chunk_pos: &Vector2<i32>,
    nearest_player_distance_sq: f64,
) -> bool {
    if nearest_player_distance_sq <= 576.0 {
        return false;
    }

    if world.dimension == Dimension::OVERWORLD {
        let level_info = world.level_info.load();
        let dx = f64::from(level_info.spawn_x - pos.0.x);
        let dy = (f64::from(level_info.spawn_y) + 0.5) - f64::from(pos.0.y);
        let dz = f64::from(level_info.spawn_z - pos.0.z);
        if dx * dx + dy * dy + dz * dz < 576.0 {
            return false;
        }
    }

    let chunk_x = get_section_cord(pos.0.x);
    let chunk_z = get_section_cord(pos.0.z);
    let target_chunk = Vector2::new(chunk_x, chunk_z);
    target_chunk == *chunk_pos || world.can_spawn_entities_in_chunk(target_chunk)
}

/// Vanilla `NetherFortressStructure.FORTRESS_ENEMIES`.
const FORTRESS_ENEMIES: &[Spawner] = &[
    Spawner {
        r#type: "minecraft:blaze",
        min_count: 2,
        max_count: 3,
        weight: 10,
    },
    Spawner {
        r#type: "minecraft:zombified_piglin",
        min_count: 4,
        max_count: 4,
        weight: 5,
    },
    Spawner {
        r#type: "minecraft:wither_skeleton",
        min_count: 5,
        max_count: 5,
        weight: 8,
    },
    Spawner {
        r#type: "minecraft:skeleton",
        min_count: 5,
        max_count: 5,
        weight: 2,
    },
    Spawner {
        r#type: "minecraft:magma_cube",
        min_count: 4,
        max_count: 4,
        weight: 3,
    },
];

/// Vanilla `NaturalSpawner.canSpawnMobAt`: whether `spawner` is still on the spawn list at `pos`.
#[must_use]
pub fn can_spawn_mob_at(
    world: &World,
    category: &'static MobCategory,
    spawner: &Spawner,
    pos: &BlockPos,
) -> bool {
    // Vanilla compares `SpawnerData` records: the entity type and the count provider.
    mobs_at(world, category, pos).iter().any(|other| {
        other.r#type == spawner.r#type
            && other.min_count == spawner.min_count
            && other.max_count == spawner.max_count
    })
}

#[must_use]
pub fn get_random_spawn_mob_at(
    world: &Arc<World>,
    category: &'static MobCategory,
    block_pos: &BlockPos,
) -> Option<&'static Spawner> {
    let biome = world.level.get_rough_biome(block_pos);
    if category == &MobCategory::WATER_AMBIENT
        && biome.has_tag(&MINECRAFT_REDUCE_WATER_AMBIENT_SPAWNS)
        && game_random().random::<f32>() < 0.98f32
    {
        None
    } else {
        choose_spawner(mobs_at(world, category, block_pos))
    }
}

/// Vanilla `NaturalSpawner.mobsAt`.
fn mobs_at(world: &World, category: &'static MobCategory, pos: &BlockPos) -> &'static [Spawner] {
    if is_in_nether_fortress_bounds(pos, world, category) {
        return FORTRESS_ENEMIES;
    }
    // Vanilla `ChunkGenerator.getMobsAt`: the first structure whose spawn override for this
    // category covers `pos` replaces the biome's list.
    let structure_spawns = world.level.read_structures_at(pos, |structures| {
        structures.iter().find_map(|start| {
            let spawn_override = Structure::get(&start.structure)
                .spawn_overrides
                .iter()
                .find(|spawn_override| spawn_override.category == category.name)?;
            let in_override_box = match spawn_override.bounding_box {
                BoundingBoxType::Piece => start.has_piece_at(pos),
                BoundingBoxType::Full => start.is_inside(pos),
            };
            in_override_box.then_some(spawn_override.spawns)
        })
    });
    if let Some(spawns) = structure_spawns.flatten() {
        return spawns;
    }
    let spawners = &world.level.get_rough_biome(pos).spawners;
    match category.id {
        id if id == MobCategory::MONSTER.id => spawners.monster,
        id if id == MobCategory::CREATURE.id => spawners.creature,
        id if id == MobCategory::AMBIENT.id => spawners.ambient,
        id if id == MobCategory::AXOLOTLS.id => spawners.axolotls,
        id if id == MobCategory::UNDERGROUND_WATER_CREATURE.id => {
            spawners.underground_water_creature
        }
        id if id == MobCategory::WATER_CREATURE.id => spawners.water_creature,
        id if id == MobCategory::WATER_AMBIENT.id => spawners.water_ambient,
        _ => spawners.misc,
    }
}

/// Vanilla `NaturalSpawner.isInNetherFortressBounds`.
#[must_use]
pub fn is_in_nether_fortress_bounds(
    pos: &BlockPos,
    world: &World,
    category: &'static MobCategory,
) -> bool {
    category == &MobCategory::MONSTER
        && world.get_block(&pos.down()) == &Block::NETHER_BRICKS
        && world
            .level
            .is_inside_structure(pos, |structure| structure == StructureKeys::Fortress)
}

/// Vanilla `WeightedList.getRandom`: one draw over the total weight, none when it is zero.
fn choose_spawner(spawners: &[Spawner]) -> Option<&Spawner> {
    let total_weight: i32 = spawners.iter().map(|spawner| spawner.weight).sum();
    if total_weight == 0 {
        return None;
    }
    let mut selection = game_random().next_bounded_i32(total_weight);
    spawners.iter().find(|spawner| {
        selection -= spawner.weight;
        selection < 0
    })
}

#[must_use]
pub fn is_valid_spawn_position_for_type(
    world: &Arc<World>,
    block_pos: &BlockPos,
    category: &'static MobCategory,
    entity_type: &'static EntityType,
    spawner: &Spawner,
    distance: f64,
    is_thundering: bool,
) -> bool {
    if category == &MobCategory::MISC || entity_type.category == &MobCategory::MISC {
        return false;
    }
    if !entity_type.can_spawn_far_from_player
        && distance
            > f64::from(entity_type.category.despawn_distance)
                * f64::from(entity_type.category.despawn_distance)
    {
        return false;
    }
    if !entity_type.summonable || !can_spawn_mob_at(world, category, spawner, block_pos) {
        return false;
    }
    if !is_spawn_position_ok(world, block_pos, entity_type) {
        return false;
    }
    if !check_spawn_rules(
        entity_type,
        world,
        block_pos,
        is_thundering,
        crate::entity::mob::spawn::EntitySpawnReason::Natural,
    ) {
        return false;
    }
    if !world.is_space_empty(entity_type.get_spawn_bounding_box(
        f64::from(block_pos.0.x) + 0.5,
        f64::from(block_pos.0.y),
        f64::from(block_pos.0.z) + 0.5,
    )) {
        return false;
    }
    true
}

#[must_use]
pub fn is_spawn_position_ok(
    world: &Arc<World>,
    block_pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    let within_bounds = world
        .worldborder
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_block(block_pos.0.x, block_pos.0.z);

    match entity_type.spawn_restriction.location {
        SpawnLocation::InLava => {
            within_bounds && world.get_fluid(block_pos).has_tag(&MINECRAFT_LAVA)
        }
        SpawnLocation::InWater => {
            if !within_bounds {
                return false;
            }
            let above_state = world.get_block_state(&block_pos.up());
            world.get_fluid(block_pos).has_tag(&MINECRAFT_WATER) && !above_state.is_solid_block()
        }
        SpawnLocation::OnGround => {
            if !within_bounds {
                return false;
            }
            let down = world.get_block_state(&block_pos.down());
            let up = world.get_block_state(&block_pos.up());
            let cur = world.get_block_state(block_pos);
            let is_valid_spawn_below = is_valid_spawn_floor(down, entity_type);

            if is_valid_spawn_below {
                is_valid_empty_spawn_block(cur, entity_type)
                    && is_valid_empty_spawn_block(up, entity_type)
            } else {
                false
            }
        }
        SpawnLocation::Unrestricted => true,
    }
}

#[must_use]
pub fn is_spawn_position_ok_cache(
    cache: &dyn GenerationCache,
    block_pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    let pos_vec = block_pos.0;
    let state = GenerationCache::get_block_state(cache, &pos_vec).to_state();

    match entity_type.spawn_restriction.location {
        SpawnLocation::InLava => {
            state.is_liquid() && Block::from_state_id(state.id).has_tag(&MINECRAFT_LAVA)
        }
        SpawnLocation::InWater => {
            let above_pos = block_pos.up().0;
            let above_state = GenerationCache::get_block_state(cache, &above_pos).to_state();

            state.is_liquid()
                && Block::from_state_id(state.id).has_tag(&MINECRAFT_WATER)
                && !above_state.is_solid_block()
        }
        SpawnLocation::OnGround => {
            let down_pos = block_pos.down().0;
            let up_pos = block_pos.up().0;

            let down = GenerationCache::get_block_state(cache, &down_pos).to_state();
            let up = GenerationCache::get_block_state(cache, &up_pos).to_state();

            let is_valid_spawn_below = is_valid_spawn_floor(down, entity_type);

            if is_valid_spawn_below {
                is_valid_empty_spawn_block(state, entity_type)
                    && is_valid_empty_spawn_block(up, entity_type)
            } else {
                false
            }
        }
        SpawnLocation::Unrestricted => true,
    }
}

#[must_use]
pub fn is_valid_empty_spawn_block(
    state: &'static BlockState,
    entity_type: &'static EntityType,
) -> bool {
    if state.is_full_cube() {
        return false;
    }
    if state.is_liquid() {
        return false;
    }
    if Block::from_state_id(state.id).has_tag(&MINECRAFT_PREVENT_MOB_SPAWNING_INSIDE) {
        return false;
    }
    !is_block_dangerous(entity_type, state)
}

#[must_use]
pub(crate) fn is_valid_spawn_floor(
    state: &'static BlockState,
    entity_type: &'static EntityType,
) -> bool {
    match Block::from_state_id(state.id).spawn_floor_predicate() {
        SpawnFloorPredicate::Default => {
            state.is_side_solid(BlockDirection::Up) && state.luminance < 14
        }
        SpawnFloorPredicate::Never => false,
        SpawnFloorPredicate::Always => true,
        SpawnFloorPredicate::OcelotOrParrot => {
            entity_type == &EntityType::OCELOT || entity_type == &EntityType::PARROT
        }
        SpawnFloorPredicate::PolarBear => entity_type == &EntityType::POLAR_BEAR,
        SpawnFloorPredicate::FireImmune => entity_type.fire_immune,
    }
}

fn is_block_dangerous(entity_type: &'static EntityType, state: &'static BlockState) -> bool {
    let block = Block::from_state_id(state.id);
    if !entity_type.fire_immune && is_burning_block(block) {
        return true;
    }
    block.id == Block::WITHER_ROSE.id
        || block.id == Block::SWEET_BERRY_BUSH.id
        || block.id == Block::CACTUS.id
        || block.id == Block::POWDER_SNOW.id
}

fn is_burning_block(block: &Block) -> bool {
    block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_FIRE)
        || block.id == Block::LAVA.id
        || block.id == Block::MAGMA_BLOCK.id
        || block.id == Block::LAVA_CAULDRON.id
        || block.id == Block::CAMPFIRE.id
        || block.id == Block::SOUL_CAMPFIRE.id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn potential_change_is_finite_beyond_i32_range() {
        let origin = BlockPos::new(0, 64, 0);
        for (far, distance) in [
            (BlockPos::new(46_342, 64, 0), 46_342.0),
            (
                BlockPos::new(40_000, 64, 40_000),
                40_000f64.hypot(40_000f64),
            ),
        ] {
            let change = PointCharge(far, 1.0).get_potential_change(&origin);

            assert_eq!(change, 1.0 / distance, "wrong potential for {far:?}");
        }
    }

    #[test]
    fn vanilla_spawn_floor_predicates_are_preserved() {
        for (block, entity_type, expected) in [
            (&Block::BEDROCK, &EntityType::CREEPER, false),
            (&Block::STONE, &EntityType::CREEPER, true),
            (&Block::GLASS, &EntityType::CREEPER, false),
            (&Block::OAK_LEAVES, &EntityType::OCELOT, true),
            (&Block::OAK_LEAVES, &EntityType::CREEPER, false),
            (&Block::ICE, &EntityType::POLAR_BEAR, true),
            (&Block::ICE, &EntityType::CREEPER, false),
            (&Block::MAGMA_BLOCK, &EntityType::BLAZE, true),
            (&Block::MAGMA_BLOCK, &EntityType::CREEPER, false),
            (&Block::JACK_O_LANTERN, &EntityType::CREEPER, true),
        ] {
            assert_eq!(
                is_valid_spawn_floor(block.default_state, entity_type),
                expected,
                "{}",
                block.name
            );
        }
    }
}
