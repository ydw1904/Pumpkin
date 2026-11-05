use std::path::PathBuf;
use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::Taggable;
use pumpkin_data::tag::WorldgenBiome::MINECRAFT_WITHOUT_WANDERING_TRADER_SPAWNS;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::world_info::MAXIMUM_SUPPORTED_WORLD_DATA_VERSION;
use pumpkin_world::world_info::data_files::{
    WanderingTraderData, read_wandering_trader, write_wandering_trader,
};
use tracing::error;

use crate::entity::mob::Mob;
use crate::entity::mob::spawn::{EntitySpawnReason, finalize_spawn};
use crate::entity::passive::trader_llama::TraderLlamaEntity;
use crate::entity::passive::wandering_trader::WanderingTraderEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::natural_spawner;
use pumpkin_data::poi_type::PoiType;

const DEFAULT_TICK_DELAY: i32 = 1200;
pub const DEFAULT_SPAWN_DELAY: i32 = 24_000;
const MIN_SPAWN_CHANCE: i32 = 25;
const MAX_SPAWN_CHANCE: i32 = 75;
const SPAWN_CHANCE_INCREASE: i32 = 25;
const SPAWN_ONE_IN_X_CHANCE: i32 = 10;
const NUMBER_OF_SPAWN_ATTEMPTS: i32 = 10;
const SPAWN_RADIUS: i32 = 48;
const LLAMA_SPAWN_RADIUS: i32 = 4;
const LLAMA_COUNT: i32 = 2;
const TRADER_DESPAWN_DELAY: i32 = 48_000;
const TRADER_HOME_RADIUS: i32 = 16;

/// `SpawnPlacements.getHeightmapType(WANDERING_TRADER)`.
const HEIGHTMAP: ChunkHeightmapType = ChunkHeightmapType::MotionBlockingNoLeaves;

/// Vanilla `WanderingTraderSpawner`, backed by the `wandering_trader` saved data.
pub struct WanderingTraderSpawner {
    tick_delay: i32,
    data: WanderingTraderData,
    data_dir: PathBuf,
}

impl WanderingTraderSpawner {
    /// Vanilla keeps `wandering_trader.dat` in the server's data storage, so `level_folder`
    /// is the world's root folder, not a dimension's.
    #[must_use]
    pub fn new(level_folder: &std::path::Path) -> Self {
        let data_dir = pumpkin_world::world_info::data_files::minecraft_data_dir(level_folder);
        let mut data = read_wandering_trader(&data_dir);
        data.data_version = MAXIMUM_SUPPORTED_WORLD_DATA_VERSION;
        Self {
            tick_delay: DEFAULT_TICK_DELAY,
            data,
            data_dir,
        }
    }

    pub fn tick(&mut self, world: &Arc<World>) {
        if !world.level_info.load().game_rules.spawn_wandering_traders {
            return;
        }

        self.tick_with(|| game_random().random_range(0..100), || Self::spawn(world));
    }

    /// Vanilla `WanderingTraderSpawner.tick`: the schedule and chance bookkeeping around one
    /// spawn attempt. `spawn` is the level-dependent placement step.
    fn tick_with(
        &mut self,
        chance_roll: impl FnOnce() -> i32,
        spawn: impl FnOnce() -> bool,
    ) -> bool {
        self.tick_delay -= 1;
        if self.tick_delay > 0 {
            return false;
        }
        self.tick_delay = DEFAULT_TICK_DELAY;

        self.set_spawn_delay(self.data.spawn_delay - DEFAULT_TICK_DELAY);
        if self.data.spawn_delay > 0 {
            return false;
        }

        self.set_spawn_delay(DEFAULT_SPAWN_DELAY);
        let spawn_chance = self.data.spawn_chance;
        self.set_spawn_chance(
            (spawn_chance + SPAWN_CHANCE_INCREASE).clamp(MIN_SPAWN_CHANCE, MAX_SPAWN_CHANCE),
        );

        if chance_roll() <= spawn_chance && spawn() {
            self.set_spawn_chance(MIN_SPAWN_CHANCE);
            return true;
        }

        false
    }

    fn set_spawn_delay(&mut self, spawn_delay: i32) {
        if self.data.spawn_delay != spawn_delay {
            self.data.spawn_delay = spawn_delay;
            self.save();
        }
    }

    fn set_spawn_chance(&mut self, spawn_chance: i32) {
        if self.data.spawn_chance != spawn_chance {
            self.data.spawn_chance = spawn_chance;
            self.save();
        }
    }

    fn save(&self) {
        if let Err(error) = write_wandering_trader(&self.data_dir, &self.data) {
            error!("Failed to save wandering trader data: {error}");
        }
    }

    fn spawn(world: &Arc<World>) -> bool {
        // Vanilla `ServerLevel.getRandomPlayer`: a random living player.
        let player_pos = {
            let players: Vec<_> = world
                .players
                .load()
                .iter()
                .filter(|player| player.living_entity.is_alive())
                .cloned()
                .collect();
            game_random()
                .choose(&players)
                .map(|player| player.get_entity().block_pos.load())
        };
        let Some(player_pos) = player_pos else {
            return true;
        };

        if !Self::passes_spawn_gate() {
            return false;
        }

        let reference_pos = Self::meeting_site_near(world, player_pos).unwrap_or(player_pos);
        let Some(spawn_pos) = Self::find_spawn_position_near(world, reference_pos, SPAWN_RADIUS)
        else {
            return false;
        };
        if !Self::has_enough_space(world, spawn_pos) {
            return false;
        }
        if world
            .get_biome(&spawn_pos)
            .has_tag(&MINECRAFT_WITHOUT_WANDERING_TRADER_SPAWNS)
        {
            return false;
        }

        let trader = WanderingTraderEntity::new(Entity::new(
            world.clone(),
            spawn_position(spawn_pos),
            &EntityType::WANDERING_TRADER,
        ));
        trader
            .get_entity()
            .set_rotation(wrap_degrees(game_random().random::<f32>() * 360.0), 0.0);
        finalize_spawn(
            &(trader.clone() as Arc<dyn EntityBase>),
            world,
            EntitySpawnReason::Event,
            None,
        );
        if !world.spawn_entity(trader.clone() as Arc<dyn EntityBase>) {
            return false;
        }

        for _ in 0..LLAMA_COUNT {
            Self::try_to_spawn_llama_for(world, &trader, LLAMA_SPAWN_RADIUS);
        }

        trader.set_despawn_delay(TRADER_DESPAWN_DELAY);
        trader.set_wander_target(Some(reference_pos));
        trader
            .get_mob_entity()
            .set_home_to(reference_pos, TRADER_HOME_RADIUS);
        true
    }

    fn try_to_spawn_llama_for(
        world: &Arc<World>,
        trader: &Arc<WanderingTraderEntity>,
        radius: i32,
    ) {
        let Some(spawn_pos) =
            Self::find_spawn_position_near(world, trader.get_entity().block_pos.load(), radius)
        else {
            return;
        };

        let llama = TraderLlamaEntity::new(Entity::new(
            world.clone(),
            spawn_position(spawn_pos),
            &EntityType::TRADER_LLAMA,
        ));
        llama
            .get_entity()
            .set_rotation(wrap_degrees(game_random().random::<f32>() * 360.0), 0.0);
        finalize_spawn(
            &(llama.clone() as Arc<dyn EntityBase>),
            world,
            EntitySpawnReason::Event,
            None,
        );
        if world.spawn_entity(llama.clone() as Arc<dyn EntityBase>) {
            llama
                .get_entity()
                .attach_leash(trader.clone() as Arc<dyn EntityBase>);
        }
    }

    /// Vanilla's 1-in-10 gate before a placement attempt.
    fn passes_spawn_gate() -> bool {
        game_random().random_range(0..SPAWN_ONE_IN_X_CHANCE) == 0
    }

    fn meeting_site_near(world: &World, player_pos: BlockPos) -> Option<BlockPos> {
        world
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .find_closest_matching(player_pos, SPAWN_RADIUS, |poi_type| {
                PoiType::from_name(poi_type) == Some(PoiType::Meeting)
            })
            .map(|(position, _)| position)
    }

    fn find_spawn_position_near(
        world: &Arc<World>,
        reference_pos: BlockPos,
        radius: i32,
    ) -> Option<BlockPos> {
        for _ in 0..NUMBER_OF_SPAWN_ATTEMPTS {
            let x = reference_pos.0.x + game_random().random_range(0..radius * 2) - radius;
            let z = reference_pos.0.z + game_random().random_range(0..radius * 2) - radius;
            // `World.get_heightmap_height` returns the top block, vanilla `Level.getHeight`
            // the first free position above it.
            let y = world.get_heightmap_height(HEIGHTMAP, x, z) + 1;
            let spawn_pos = BlockPos::new(x, y, z);
            if natural_spawner::is_spawn_position_ok(
                world,
                &spawn_pos,
                &EntityType::WANDERING_TRADER,
            ) {
                return Some(spawn_pos);
            }
        }
        None
    }

    fn has_enough_space(world: &World, spawn_pos: BlockPos) -> bool {
        BlockPos::between_closed(spawn_pos, spawn_pos.offset(Vector3::new(1, 2, 1))).all(|pos| {
            world
                .get_block_state(&pos)
                .get_block_collision_shapes_at(&pos)
                .next()
                .is_none()
        })
    }
}

fn spawn_position(pos: BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}
