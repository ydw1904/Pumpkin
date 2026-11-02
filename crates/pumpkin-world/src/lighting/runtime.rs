use crate::chunk::io::Dirtiable;
use crate::chunk::palette::BlockPalette;
use crate::level::Level;
use crate::lighting::engine::{
    LayerLightEngine, LightLayer, LightStorage, find_lowest_sky_source_y,
};
use pumpkin_config::lighting::LightingEngineConfig;
use pumpkin_data::{BlockState, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use std::sync::Arc;

/// Light updates for block changes in loaded chunks.
pub struct DynamicLightEngine {
    min_y: i32,
    max_y: i32,
}

/// Light access through the level's loaded chunks.
struct LevelLightStorage<'a> {
    level: &'a Level,
    min_y: i32,
    max_y: i32,
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
    pub const fn new(min_y: i32, max_y: i32) -> Self {
        Self { min_y, max_y }
    }

    /// Updates block and sky light after the block at `pos` changed, like vanilla
    /// `LevelLightEngine.checkBlock` followed by `runLightUpdates`.
    pub fn update_lighting_at(&self, level: &Arc<Level>, pos: BlockPos) {
        self.update_lighting(level, &[pos]);
    }

    /// Updates light after several blocks changed, checking all of them before propagating like
    /// vanilla does for the block checks queued between two light updates.
    pub fn update_lighting(&self, level: &Arc<Level>, positions: &[BlockPos]) {
        let fixed_level = match level.lighting_config {
            LightingEngineConfig::Full => Some(15),
            LightingEngineConfig::Dark => Some(0),
            LightingEngineConfig::Default => None,
        };
        if let Some(value) = fixed_level {
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
