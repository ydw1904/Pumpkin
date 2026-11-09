use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::statistic::{CustomStatistic, StatisticCategory};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::gameplay::game_random;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::r#type::from_type;
use crate::world::{World, natural_spawner};

/// Vanilla `Stats.TIME_SINCE_REST` needed before phantoms can appear: three days.
const MIN_TIME_SINCE_REST: i32 = 72_000;

/// Vanilla `PhantomSpawner`: insomniac players under the open night sky get visited by phantoms.
#[derive(Default)]
pub struct PhantomSpawner {
    next_tick: i32,
}

impl PhantomSpawner {
    pub fn tick(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if !spawn_enemies || !world.level_info.load().game_rules.spawn_phantoms {
            return;
        }
        let random = game_random();
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick += (60 + random.random_range(0..60)) * 20;
        let has_sky_light = world.dimension.has_skylight;
        if world.get_sky_darken() < 5 && has_sky_light {
            return;
        }

        for player in world.players.load().iter() {
            if player.is_spectator() {
                continue;
            }
            let player_pos = player.get_entity().block_pos.load();
            if has_sky_light
                && (player_pos.0.y < world.sea_level || !world.can_see_sky(&player_pos))
            {
                continue;
            }
            let difficulty = RegionalDifficulty::at(world, player_pos.to_f64());
            if difficulty.effective_difficulty <= random.random::<f32>() * 3.0 {
                continue;
            }
            let time_since_rest = player
                .stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(
                    StatisticCategory::Custom,
                    CustomStatistic::TimeSinceRest as i32,
                )
                .max(1);
            if random.random_range(0..time_since_rest) < MIN_TIME_SINCE_REST {
                continue;
            }

            let up = 20 + random.random_range(0..15);
            let east = -10 + random.random_range(0..21);
            let south = -10 + random.random_range(0..21);
            let spawn_pos = BlockPos::new(
                player_pos.0.x + east,
                player_pos.0.y + up,
                player_pos.0.z + south,
            );
            if !natural_spawner::is_valid_empty_spawn_block_at(
                &world.block_registry,
                world.get_block_state(&spawn_pos),
                &EntityType::PHANTOM,
            ) {
                continue;
            }

            let group_size = 1 + random.random_range(0..difficulty.base_difficulty as i32 + 1);
            let mut group_data = None;
            for _ in 0..group_size {
                let phantom = from_type(
                    &EntityType::PHANTOM,
                    Vector3::new(
                        f64::from(spawn_pos.0.x) + 0.5,
                        f64::from(spawn_pos.0.y),
                        f64::from(spawn_pos.0.z) + 0.5,
                    ),
                    world,
                    Uuid::new_v4(),
                );
                group_data = finalize_spawn(
                    &phantom,
                    world,
                    crate::entity::mob::spawn::EntitySpawnReason::Natural,
                    group_data,
                );
                world.spawn_entity(phantom as Arc<dyn EntityBase>);
            }
        }
    }
}
