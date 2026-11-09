pub mod light;
pub mod util;
pub mod v1_18;

pub use light::ChunkLightExt;

use pumpkin_data::packet::clientbound::play::LEVEL_CHUNK_WITH_LIGHT;
use pumpkin_protocol::ClientPacket;
use pumpkin_protocol::packet::MultiVersionJavaPacket;
use pumpkin_protocol::ser::WritingError;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::level::Level;
use std::io::Write;

/// Sent by the server to provide the client with the full data for a chunk.
///
/// This includes heightmaps, the actual block and biome data (organized into sections),
/// block entities (like signs or chests), and the light level information for both
/// sky and block light.
pub struct CChunkData<'a> {
    chunk: &'a ChunkData,
    /// The light sections sent, from [`light::light_sections_with_data`].
    light_sections: Vec<bool>,
}

impl MultiVersionJavaPacket for CChunkData<'_> {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        LEVEL_CHUNK_WITH_LIGHT.to_id(version)
    }
}

impl<'a> CChunkData<'a> {
    /// The chunk's packet, with light for the sections vanilla keeps light for, which depends
    /// on the neighbouring chunks loaded in `level`.
    pub fn new(chunk: &'a ChunkData, level: &Level) -> Result<Self, WritingError> {
        Ok(Self {
            chunk,
            light_sections: light::light_sections_with_data(level, chunk)?,
        })
    }
}

impl ClientPacket for CChunkData<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        v1_18::write_chunk_data(self.chunk, &self.light_sections, write, version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_world::chunk::ChunkData;

    #[test]
    fn chunk_data_all_versions() {
        let chunk = ChunkData::empty(0, 0);
        let packet = CChunkData {
            chunk: &chunk,
            light_sections: vec![false; chunk.section.count + 2],
        };

        let versions = [JavaMinecraftVersion::V_26_3];

        for version in versions {
            let mut buf = Vec::new();
            let id = CChunkData::to_id(version);
            assert_ne!(id, -1, "Packet ID for version {version:?} must be valid");
            assert!(
                packet.write_packet_data(&mut buf, &version).is_ok(),
                "Failed to serialize chunk data for version {version:?}"
            );
            assert!(
                !buf.is_empty(),
                "Serialized buffer must not be empty for version {version:?}"
            );
        }
    }

    #[test]
    fn populated_chunk_data_all_versions() {
        let chunk = ChunkData::empty(0, 0);
        chunk
            .section
            .set_block_absolute_y(0, 64, 0, pumpkin_data::Block::STONE.default_state.id);
        chunk
            .section
            .set_block_absolute_y(1, 64, 1, pumpkin_data::Block::DIRT.default_state.id);

        let mut nbt = pumpkin_nbt::compound::NbtCompound::new();
        nbt.put_string("id", "minecraft:chest".to_string());
        chunk.pending_block_entities.lock().unwrap().insert(
            pumpkin_util::math::position::BlockPos(pumpkin_util::math::vector3::Vector3::new(
                0, 64, 0,
            )),
            nbt,
        );

        let packet = CChunkData {
            chunk: &chunk,
            light_sections: vec![false; chunk.section.count + 2],
        };

        let versions = [JavaMinecraftVersion::V_26_3];

        for version in versions {
            let mut buf = Vec::new();
            let id = CChunkData::to_id(version);
            assert_ne!(id, -1, "Packet ID for version {version:?} must be valid");
            assert!(
                packet.write_packet_data(&mut buf, &version).is_ok(),
                "Failed to serialize populated chunk data for version {version:?}"
            );
            assert!(
                !buf.is_empty(),
                "Serialized buffer must not be empty for version {version:?}"
            );
        }
    }

    #[tokio::test]
    async fn light_covers_sections_next_to_blocks_in_neighbouring_chunks() {
        use pumpkin_config::world::LevelConfig;
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::math::vector2::Vector2;
        use pumpkin_world::chunk::format::LightContainer;

        let directory = tempfile::TempDir::new().unwrap();
        let level = Level::from_root_folder(
            &LevelConfig::default(),
            directory.path().to_path_buf(),
            0,
            Dimension::OVERWORLD,
        );
        let lit = |x, z| {
            let chunk = ChunkData::empty(x, z);
            let mut light = chunk.light_engine.lock().unwrap();
            light.sky_light = vec![LightContainer::new_filled(15); 24].into();
            light.block_light = vec![LightContainer::Empty(0); 24].into();
            drop(light);
            std::sync::Arc::new(chunk)
        };
        let with_stone = lit(0, 0);
        with_stone.section.set_block_absolute_y(
            0,
            64,
            0,
            pumpkin_data::Block::STONE.default_state.id,
        );
        level
            .loaded_chunks
            .insert(Vector2::new(0, 0), with_stone.clone());
        let beside = lit(1, 1);
        level
            .loaded_chunks
            .insert(Vector2::new(1, 1), beside.clone());
        let away = lit(2, 0);

        // y = 64 is in world section 8, which is light section 9.
        let near = 0b111 << 8;
        for (chunk, mask) in [(&with_stone, near), (&beside, near), (&away, 0)] {
            let sections = light::light_sections_with_data(&level, chunk).unwrap();
            let data = light::light_data_from_chunk(chunk, &sections, JavaMinecraftVersion::V_26_3)
                .unwrap();
            assert_eq!(data.sky_light_mask.as_u64(), mask);
            assert_eq!(data.empty_sky_light_mask.as_u64(), 0);
            assert_eq!(data.block_light_mask.as_u64(), 0);
            assert_eq!(data.empty_block_light_mask.as_u64(), mask);
        }
        level.shutdown().await;
    }
}
