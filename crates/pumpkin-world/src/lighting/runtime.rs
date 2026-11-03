use crate::chunk::io::Dirtiable;
use crate::chunk::palette::BlockPalette;
use crate::level::Level;
use crate::lighting::engine::{
    LayerLightEngine, LightLayer, LightStorage, find_lowest_sky_source_y,
};
use pumpkin_data::{BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

/// Light updates for block changes in loaded chunks.
pub struct DynamicLightEngine {
    min_y: i32,
    max_y: i32,
    /// Positions in chunks that weren't loaded yet when light tried to reach them, checked again
    /// once the chunk is loaded. Vanilla's light engine already reaches lit chunks that aren't
    /// full, but Pumpkin's runtime light only sees loaded chunks.
    pending_checks: Mutex<FxHashMap<Vector2<i32>, FxHashSet<BlockPos>>>,
}

/// Light access through the level's loaded chunks.
struct LevelLightStorage<'a> {
    level: &'a Level,
    min_y: i32,
    max_y: i32,
    /// Positions light couldn't reach because their chunk isn't loaded.
    unloaded: RefCell<Vec<BlockPos>>,
}

impl LightStorage for LevelLightStorage<'_> {
    fn get_level(&self, layer: LightLayer, pos: BlockPos) -> Option<u8> {
        if pos.0.y < self.min_y || pos.0.y >= self.max_y {
            return None;
        }
        let (chunk_pos, relative) = pos.chunk_and_chunk_relative_position();
        self.level
            .read_chunk_sync(&chunk_pos, |chunk| {
                let local_y = (relative.y - chunk.section.min_y) as usize;
                let light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let sections = match layer {
                    LightLayer::Sky => &light_engine.sky_light,
                    LightLayer::Block => &light_engine.block_light,
                };
                sections.get(local_y / BlockPalette::SIZE).map(|section| {
                    section.get(
                        relative.x as usize,
                        local_y % BlockPalette::SIZE,
                        relative.z as usize,
                    )
                })
            })
            .or_else(|| {
                self.unloaded.borrow_mut().push(pos);
                None
            })
            .flatten()
    }

    fn set_level(&mut self, layer: LightLayer, pos: BlockPos, level: u8) {
        let (chunk_pos, relative) = pos.chunk_and_chunk_relative_position();
        self.level.read_chunk_sync(&chunk_pos, |chunk| {
            let local_y = (relative.y - chunk.section.min_y) as usize;
            let mut light_engine = chunk
                .light_engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let sections = match layer {
                LightLayer::Sky => &mut light_engine.sky_light,
                LightLayer::Block => &mut light_engine.block_light,
            };
            let Some(section) = sections.get_mut(local_y / BlockPalette::SIZE) else {
                return;
            };
            section.set(
                relative.x as usize,
                local_y % BlockPalette::SIZE,
                relative.z as usize,
                level,
            );
            drop(light_engine);
            if !chunk.is_dirty() {
                chunk.mark_dirty(true);
            }
        });
    }

    fn get_state(&self, pos: BlockPos) -> &'static BlockState {
        self.level.get_block_state(&pos).to_state()
    }

    fn lowest_sky_source_y(&self, x: i32, z: i32, default: i32) -> i32 {
        let (local_x, local_z) = ((x & 15) as usize, (z & 15) as usize);
        self.level
            .read_chunk_sync(&Vector2::new(x >> 4, z >> 4), |chunk| {
                let sections = chunk
                    .section
                    .block_sections
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Only scan from the highest section with blocks; air never stops sky light.
                let top_y = sections
                    .iter()
                    .rposition(|section| !section.has_only_air())
                    .map_or(self.min_y, |index| {
                        chunk.section.min_y + (index as i32 + 1) * BlockPalette::SIZE as i32
                    });
                find_lowest_sky_source_y(self.min_y, top_y, |y| {
                    let local_y = (y - chunk.section.min_y) as usize;
                    sections
                        .get(local_y / BlockPalette::SIZE)
                        .map_or(BlockStateId::AIR, |section| {
                            section.get(local_x, local_y % BlockPalette::SIZE, local_z)
                        })
                        .to_state()
                })
            })
            .unwrap_or(default)
    }

    fn bottom_y(&self) -> i32 {
        self.min_y
    }

    fn top_y(&self) -> i32 {
        self.max_y
    }
}

impl DynamicLightEngine {
    #[must_use]
    pub fn new(min_y: i32, max_y: i32) -> Self {
        Self {
            min_y,
            max_y,
            pending_checks: Mutex::default(),
        }
    }

    /// Checks the light that couldn't reach `chunk` before it was loaded, like vanilla would
    /// have when the change happened.
    pub fn check_pending(&self, level: &Arc<Level>, chunk: Vector2<i32>) {
        let positions = self
            .pending_checks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&chunk);
        if let Some(positions) = positions {
            self.update_lighting(level, &positions.into_iter().collect::<Vec<_>>());
        }
    }

    /// Updates block and sky light after the block at `pos` changed, like vanilla
    /// `LevelLightEngine.checkBlock` followed by `runLightUpdates`.
    pub fn update_lighting_at(&self, level: &Arc<Level>, pos: BlockPos) {
        self.update_lighting(level, &[pos]);
    }

    /// Updates light after several blocks changed, checking all of them before propagating like
    /// vanilla does for the block checks queued between two light updates.
    pub fn update_lighting(&self, level: &Arc<Level>, positions: &[BlockPos]) {
        if let Some(value) = level.lighting_config.fixed_level() {
            for pos in positions {
                self.set_block_light_level(level, pos, value).ok();
                self.set_sky_light_level(level, pos, value).ok();
            }
            return;
        }

        let mut storage = LevelLightStorage {
            level,
            min_y: self.min_y,
            max_y: self.max_y,
            unloaded: RefCell::default(),
        };

        let mut block_light = LayerLightEngine::new(LightLayer::Block);
        for &pos in positions {
            block_light.check_node(&mut storage, pos);
        }
        block_light.run_updates(&mut storage);

        if level.world_gen.load().dimension().has_skylight {
            let mut sky_light = LayerLightEngine::new(LightLayer::Sky);
            for &pos in positions {
                sky_light.check_node(&mut storage, pos);
            }
            sky_light.run_updates(&mut storage);
        }

        let unloaded = storage.unloaded.into_inner();
        if unloaded.is_empty() {
            return;
        }
        let mut chunks = FxHashSet::default();
        {
            let mut pending = self
                .pending_checks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for pos in unloaded {
                let chunk = pos.chunk_and_chunk_relative_position().0;
                pending.entry(chunk).or_default().insert(pos);
                chunks.insert(chunk);
            }
        }
        // A chunk that got loaded meanwhile won't be announced again.
        for chunk in chunks {
            if level.is_chunk_loaded(&chunk) {
                self.check_pending(level, chunk);
            }
        }
    }

    pub fn get_block_light_level_sync(&self, level: &Level, position: &BlockPos) -> Option<u8> {
        let (chunk_pos, relative) = position.chunk_and_chunk_relative_position();

        level.read_chunk_sync(&chunk_pos, |chunk| {
            let section_idx = (relative.y - chunk.section.min_y) as usize / 16;
            let light_engine = chunk.light_engine.lock().ok()?;

            light_engine
                .block_light
                .get(section_idx)?
                .get(
                    relative.x as usize,
                    (relative.y - chunk.section.min_y) as usize % 16,
                    relative.z as usize,
                )
                .into()
        })?
    }

    pub fn get_sky_light_level_sync(&self, level: &Level, position: &BlockPos) -> u8 {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level
            .read_chunk_sync(&chunk_coordinate, |chunk| {
                let section_index =
                    (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;

                let light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Bounds check for section index (lock the light engine)
                if section_index >= light_engine.sky_light.len() {
                    return 15;
                }

                light_engine.sky_light[section_index].get(
                    relative.x as usize,
                    (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE,
                    relative.z as usize,
                )
            })
            .unwrap_or(0)
    }

    pub fn get_block_light_level(&self, level: &Arc<Level>, position: &BlockPos) -> Option<u8> {
        let (chunk_pos, relative) = position.chunk_and_chunk_relative_position();

        level
            .read_chunk_sync(&chunk_pos, |chunk| {
                let section_idx = (relative.y - chunk.section.min_y) as usize / 16;
                chunk
                    .light_engine
                    .lock()
                    .ok()?
                    .block_light
                    .get(section_idx)
                    .map(|section| {
                        section.get(
                            relative.x as usize,
                            (relative.y - chunk.section.min_y) as usize % 16,
                            relative.z as usize,
                        )
                    })
            })
            .flatten()
    }

    pub fn set_block_light_level(
        &self,
        level: &Arc<Level>,
        position: &BlockPos,
        light_level: u8,
    ) -> Result<(), String> {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level.read_chunk_sync(&chunk_coordinate, |chunk| {
            let section_index = (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;
            {
                let mut light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if section_index >= light_engine.block_light.len() {
                    return Err("Invalid section index".to_string());
                }
                let relative_y = (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE;
                light_engine.block_light[section_index].set(
                    relative.x as usize,
                    relative_y,
                    relative.z as usize,
                    light_level,
                );
            };
            // Mark chunk as dirty so lighting changes are saved to disk
            if !chunk.is_dirty() {
                chunk.mark_dirty(true);
            }
            Ok(())
        });
        Ok(())
    }

    pub fn get_sky_light_level(&self, level: &Arc<Level>, position: &BlockPos) -> u8 {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level
            .read_chunk_sync(&chunk_coordinate, |chunk| {
                let section_index =
                    (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;

                let light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Bounds check for section index (lock the light engine)
                if section_index >= light_engine.sky_light.len() {
                    return 15;
                }

                light_engine.sky_light[section_index].get(
                    relative.x as usize,
                    (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE,
                    relative.z as usize,
                )
            })
            .unwrap_or(0)
    }

    pub fn set_sky_light_level(
        &self,
        level: &Arc<Level>,
        position: &BlockPos,
        light_level: u8,
    ) -> Result<(), String> {
        let (chunk_coordinate, relative) = position.chunk_and_chunk_relative_position();
        level.read_chunk_sync(&chunk_coordinate, |chunk| {
            let section_index = (relative.y - chunk.section.min_y) as usize / BlockPalette::SIZE;
            {
                let mut light_engine = chunk
                    .light_engine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if section_index >= light_engine.sky_light.len() {
                    return Err("Invalid section index".to_string());
                }
                let relative_y = (relative.y - chunk.section.min_y) as usize % BlockPalette::SIZE;
                light_engine.sky_light[section_index].set(
                    relative.x as usize,
                    relative_y,
                    relative.z as usize,
                    light_level,
                );
            };
            // Mark chunk as dirty so lighting changes are saved to disk
            if !chunk.is_dirty() {
                chunk.mark_dirty(true);
            }
            Ok(())
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::format::LightContainer;
    use crate::chunk::{ChunkData, ChunkLight};
    use crate::level::SyncChunk;
    use pumpkin_config::world::LevelConfig;
    use pumpkin_data::Block;
    use pumpkin_data::dimension::Dimension;
    use tempfile::TempDir;

    fn lit_chunk(x: i32, z: i32) -> SyncChunk {
        let mut chunk = ChunkData::empty(x, z);
        let sections = chunk.section.count;
        chunk.light_engine = Mutex::new(ChunkLight {
            sky_light: (0..sections)
                .map(|_| LightContainer::new_empty(15))
                .collect(),
            block_light: (0..sections)
                .map(|_| LightContainer::new_empty(0))
                .collect(),
        });
        Arc::new(chunk)
    }

    #[tokio::test]
    async fn light_reaches_chunks_loaded_after_the_change() {
        let temp_dir = TempDir::new().unwrap();
        let level = Level::from_root_folder(
            &LevelConfig::default(),
            temp_dir.path().to_path_buf(),
            0,
            Dimension::OVERWORLD,
        );
        let origin = Vector2::new(0, 0);
        let neighbor = Vector2::new(1, 0);
        let chunk = lit_chunk(origin.x, origin.y);
        chunk.set_block_absolute_y(15, 64, 8, Block::GLOWSTONE.default_state.id);
        level.loaded_chunks.insert(origin, chunk);
        level
            .light_engine
            .update_lighting_at(&level, BlockPos::new(15, 64, 8));

        let across = BlockPos::new(16, 64, 8);
        level
            .loaded_chunks
            .insert(neighbor, lit_chunk(neighbor.x, neighbor.y));
        assert_eq!(
            level.light_engine.get_block_light_level(&level, &across),
            Some(0)
        );
        level.light_engine.check_pending(&level, neighbor);
        assert_eq!(
            level.light_engine.get_block_light_level(&level, &across),
            Some(14)
        );
        level.shutdown().await;
    }
}
