use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::random::gameplay::game_random;
use pumpkin_world::chunk::ChunkHeightmapType;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::r#type::{check_spawn_rules, from_type};
use crate::world::{World, natural_spawner};

/// Vanilla `PatrolSpawner`: by day, now and then a pillager patrol appears near a player who is
/// away from villages.
#[derive(Default)]
pub struct PatrolSpawner {
    next_tick: i32,
}

impl PatrolSpawner {
    pub fn tick(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if !spawn_enemies || !world.level_info.load().game_rules.spawn_patrols {
            return;
        }
        let random = game_random();
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick += 12000 + random.random_range(0..1200);
        if !world.is_bright_outside() || random.random_range(0..5) != 0 {
            return;
        }
        let players = world.players.load();
        if players.is_empty() {
            return;
        }
        let player = &players[random.random_range(0..players.len())];
        if player.is_spectator() {
            return;
        }
        let player_pos = player.get_entity().block_pos.load();
        if world.is_close_to_village(&player_pos, 2) {
            return;
        }

        let x = (24 + random.random_range(0..24)) * if random.random::<bool>() { -1 } else { 1 };
        let z = (24 + random.random_range(0..24)) * if random.random::<bool>() { -1 } else { 1 };
        let mut spawn_pos = BlockPos::new(player_pos.0.x + x, player_pos.0.y, player_pos.0.z + z);
        if !Self::has_chunks_around(world, spawn_pos, 10)
            || !world.can_pillager_patrol_spawn(&spawn_pos)
        {
            return;
        }

        let difficulty = RegionalDifficulty::at(world, spawn_pos.to_f64());
        let group_size = difficulty.effective_difficulty.ceil() as i32 + 1;
        for i in 0..group_size {
            spawn_pos.0.y = world.get_heightmap_height(
                ChunkHeightmapType::MotionBlockingNoLeaves,
                spawn_pos.0.x,
                spawn_pos.0.z,
            ) + 1;
            if i == 0 {
                if !Self::spawn_patrol_member(world, spawn_pos, true) {
                    break;
                }
            } else {
                Self::spawn_patrol_member(world, spawn_pos, false);
            }
            spawn_pos.0.x += random.random_range(0..5) - random.random_range(0..5);
            spawn_pos.0.z += random.random_range(0..5) - random.random_range(0..5);
        }
    }

    /// Vanilla `Level.hasChunksAt` for the square of `radius` blocks around `pos`.
    fn has_chunks_around(world: &World, pos: BlockPos, radius: i32) -> bool {
        let (min_x, max_x) = ((pos.0.x - radius) >> 4, (pos.0.x + radius) >> 4);
        let (min_z, max_z) = ((pos.0.z - radius) >> 4, (pos.0.z + radius) >> 4);
        (min_x..=max_x)
            .all(|x| (min_z..=max_z).all(|z| world.level.is_chunk_loaded(&Vector2::new(x, z))))
    }

    fn spawn_patrol_member(world: &Arc<World>, pos: BlockPos, is_leader: bool) -> bool {
        if !natural_spawner::is_valid_empty_spawn_block(
            world.get_block_state(&pos),
            &EntityType::PILLAGER,
        ) || !check_spawn_rules(
            &EntityType::PILLAGER,
            world,
            &pos,
            world.is_thundering(),
            crate::entity::mob::spawn::EntitySpawnReason::Patrol,
        ) {
            return false;
        }
        let pillager = from_type(&EntityType::PILLAGER, pos.to_f64(), world, Uuid::new_v4());
        let Some(patroller) = pillager
            .get_mob()
            .and_then(|mob| mob.as_patrolling_monster())
        else {
            return false;
        };
        if is_leader {
            patroller.set_patrol_leader(true);
            // Vanilla calls `findPatrolTarget` before placing the new mob, so the target is
            // picked around the world origin.
            let random = game_random();
            let x = -500 + random.random_range(0..1000);
            let z = -500 + random.random_range(0..1000);
            patroller.set_patrol_target(BlockPos::new(x, 0, z));
        }
        finalize_spawn(
            &pillager,
            world,
            crate::entity::mob::spawn::EntitySpawnReason::Patrol,
            None,
        );
        patroller.finalize_patrol_spawn(true);
        world.spawn_entity(pillager as Arc<dyn EntityBase>);
        true
    }
}
