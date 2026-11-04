use pumpkin_protocol::codec::bit_set::BitSet;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CLightUpdate, LightData};
use pumpkin_protocol::ser::WritingError;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::lighting::runtime::ChangedLightSections;

pub trait ChunkLightExt {
    fn from_chunk(chunk: &ChunkData, version: JavaMinecraftVersion) -> Result<Self, WritingError>
    where
        Self: Sized;
}

impl ChunkLightExt for CLightUpdate {
    fn from_chunk(chunk: &ChunkData, version: JavaMinecraftVersion) -> Result<Self, WritingError> {
        let light_data = light_data_from_chunk(chunk, version)?;
        Ok(Self {
            chunk_x: VarInt(chunk.x),
            chunk_z: VarInt(chunk.z),
            light_data,
        })
    }
}

#[expect(clippy::too_many_lines)]
pub fn light_data_from_chunk(
    chunk: &ChunkData,
    version: JavaMinecraftVersion,
) -> Result<LightData, WritingError> {
    const BELOW_WORLD: LightContainer = LightContainer::Empty(0);
    const ABOVE_WORLD_SKY: LightContainer = LightContainer::Empty(15);
    let has_blocks: Vec<bool> = chunk
        .section
        .block_sections
        .read()
        .map_err(|_| WritingError::Message("block_sections lock poisoned".into()))?
        .iter()
        .map(|section| !section.has_only_air())
        .collect();
    let light_engine = chunk
        .light_engine
        .lock()
        .map_err(|_| WritingError::Message("light_engine lock poisoned".into()))?;

    if version < JavaMinecraftVersion::V_1_18 {
        let base_section = (0 - chunk.section.min_y).max(0) as usize / 16;
        let mut sky_light_mask = 0u64;
        let mut block_light_mask = 0u64;
        let mut sky_light_empty_mask = 0u64;
        let mut block_light_empty_mask = 0u64;
        let mut sky_light_arrays = Vec::new();
        let mut block_light_arrays = Vec::new();

        // Bit 0: Y = -1 (below world section 0)
        if base_section > 0 && base_section - 1 < light_engine.sky_light.len() {
            match &light_engine.sky_light[base_section - 1] {
                LightContainer::Full(data) => {
                    sky_light_mask |= 1 << 0;
                    sky_light_arrays.push(data.to_vec());
                }
                LightContainer::Empty(val) if *val > 0 => {
                    sky_light_mask |= 1 << 0;
                    sky_light_arrays.push(vec![*val << 4 | *val; 2048]);
                }
                LightContainer::Empty(_) => {
                    sky_light_empty_mask |= 1 << 0;
                }
            }
        } else {
            sky_light_empty_mask |= 1 << 0;
        }

        if base_section > 0 && base_section - 1 < light_engine.block_light.len() {
            match &light_engine.block_light[base_section - 1] {
                LightContainer::Full(data) => {
                    block_light_mask |= 1 << 0;
                    block_light_arrays.push(data.to_vec());
                }
                LightContainer::Empty(val) if *val > 0 => {
                    block_light_mask |= 1 << 0;
                    block_light_arrays.push(vec![*val << 4 | *val; 2048]);
                }
                LightContainer::Empty(_) => {
                    block_light_empty_mask |= 1 << 0;
                }
            }
        } else {
            block_light_empty_mask |= 1 << 0;
        }

        // Bits 1..=16: world sections (Y = 0..15)
        for i in 0..16 {
            let bit_index = i + 1;
            let sec_idx = base_section + i;

            if sec_idx < light_engine.sky_light.len() {
                match &light_engine.sky_light[sec_idx] {
                    LightContainer::Full(data) => {
                        sky_light_mask |= 1 << bit_index;
                        sky_light_arrays.push(data.to_vec());
                    }
                    LightContainer::Empty(val) if *val > 0 => {
                        sky_light_mask |= 1 << bit_index;
                        sky_light_arrays.push(vec![*val << 4 | *val; 2048]);
                    }
                    LightContainer::Empty(_) => {
                        sky_light_empty_mask |= 1 << bit_index;
                    }
                }
            } else {
                sky_light_empty_mask |= 1 << bit_index;
            }

            if sec_idx < light_engine.block_light.len() {
                match &light_engine.block_light[sec_idx] {
                    LightContainer::Full(data) => {
                        block_light_mask |= 1 << bit_index;
                        block_light_arrays.push(data.to_vec());
                    }
                    LightContainer::Empty(val) if *val > 0 => {
                        block_light_mask |= 1 << bit_index;
                        block_light_arrays.push(vec![*val << 4 | *val; 2048]);
                    }
                    LightContainer::Empty(_) => {
                        block_light_empty_mask |= 1 << bit_index;
                    }
                }
            } else {
                block_light_empty_mask |= 1 << bit_index;
            }
        }

        // Bit 17: Y = 16 (above world section 15)
        let top_sec = base_section + 16;
        if top_sec < light_engine.sky_light.len() {
            match &light_engine.sky_light[top_sec] {
                LightContainer::Full(data) => {
                    sky_light_mask |= 1 << 17;
                    sky_light_arrays.push(data.to_vec());
                }
                LightContainer::Empty(val) if *val > 0 => {
                    sky_light_mask |= 1 << 17;
                    sky_light_arrays.push(vec![*val << 4 | *val; 2048]);
                }
                LightContainer::Empty(_) => {
                    sky_light_empty_mask |= 1 << 17;
                }
            }
        } else {
            sky_light_empty_mask |= 1 << 17;
        }

        if top_sec < light_engine.block_light.len() {
            match &light_engine.block_light[top_sec] {
                LightContainer::Full(data) => {
                    block_light_mask |= 1 << 17;
                    block_light_arrays.push(data.to_vec());
                }
                LightContainer::Empty(val) if *val > 0 => {
                    block_light_mask |= 1 << 17;
                    block_light_arrays.push(vec![*val << 4 | *val; 2048]);
                }
                LightContainer::Empty(_) => {
                    block_light_empty_mask |= 1 << 17;
                }
            }
        } else {
            block_light_empty_mask |= 1 << 17;
        }

        Ok(LightData {
            trust_edges: true,
            sky_light_mask: BitSet::from_u64(sky_light_mask),
            block_light_mask: BitSet::from_u64(block_light_mask),
            empty_sky_light_mask: BitSet::from_u64(sky_light_empty_mask),
            empty_block_light_mask: BitSet::from_u64(block_light_empty_mask),
            sky_light_arrays,
            block_light_arrays,
        })
    } else {
        let num_sections = light_engine.sky_light.len();
        let mut data = LightData {
            trust_edges: true,
            ..LightData::default()
        };
        let mut sky_light_mask = 0u64;
        let mut block_light_mask = 0u64;
        let mut empty_sky_light_mask = 0u64;
        let mut empty_block_light_mask = 0u64;
        // Vanilla only keeps light for sections with blocks and the sections next to them
        // (the `LayerLightSectionStorage` section states) and sends nothing for the others.
        // The client takes missing sky light above the terrain as full, so a far section is
        // left out when it holds the default light. Neighbouring chunks also count in vanilla,
        // which only adds sections with that same default light.
        let near_blocks = |light_index: usize| {
            (light_index.saturating_sub(2)..=light_index)
                .any(|section| has_blocks.get(section).copied().unwrap_or(false))
        };
        // Light section 0 is the one below the world, `num_sections + 1` the one above it.
        for light_index in 0..num_sections + 2 {
            let (sky, block, outside_world) = match light_index.checked_sub(1) {
                Some(section) if section < num_sections => (
                    &light_engine.sky_light[section],
                    &light_engine.block_light[section],
                    false,
                ),
                Some(_) => (&ABOVE_WORLD_SKY, &BELOW_WORLD, true),
                None => (&BELOW_WORLD, &BELOW_WORLD, true),
            };
            let near = near_blocks(light_index);
            let bit = 1 << light_index;
            if near || !(outside_world || sky.is_filled_with(15)) {
                push_section(
                    sky,
                    bit,
                    &mut sky_light_mask,
                    &mut empty_sky_light_mask,
                    &mut data.sky_light_arrays,
                );
            }
            if near || !(outside_world || block.is_filled_with(0)) {
                push_section(
                    block,
                    bit,
                    &mut block_light_mask,
                    &mut empty_block_light_mask,
                    &mut data.block_light_arrays,
                );
            }
        }
        data.sky_light_mask = BitSet::from_u64(sky_light_mask);
        data.block_light_mask = BitSet::from_u64(block_light_mask);
        data.empty_sky_light_mask = BitSet::from_u64(empty_sky_light_mask);
        data.empty_block_light_mask = BitSet::from_u64(empty_block_light_mask);
        Ok(data)
    }
}

/// Vanilla `ClientboundLightUpdatePacketData` for the sections in `changed` only.
pub fn changed_light_data(
    chunk: &ChunkData,
    changed: &ChangedLightSections,
) -> Result<LightData, WritingError> {
    let light_engine = chunk
        .light_engine
        .lock()
        .map_err(|_| WritingError::Message("light_engine lock poisoned".into()))?;
    let min_section = chunk.section.min_y >> 4;
    let mut data = LightData::default();
    let mut sky_light_mask = 0u64;
    let mut block_light_mask = 0u64;
    let mut empty_sky_light_mask = 0u64;
    let mut empty_block_light_mask = 0u64;
    // Light section 0 is the one below the world, like in the chunk packet.
    for (index, (sky, block)) in light_engine
        .sky_light
        .iter()
        .zip(light_engine.block_light.iter())
        .enumerate()
    {
        let section_y = min_section + index as i32;
        let bit = 1 << (index + 1);
        if changed.sky.contains(&section_y) {
            push_section(
                sky,
                bit,
                &mut sky_light_mask,
                &mut empty_sky_light_mask,
                &mut data.sky_light_arrays,
            );
        }
        if changed.block.contains(&section_y) {
            push_section(
                block,
                bit,
                &mut block_light_mask,
                &mut empty_block_light_mask,
                &mut data.block_light_arrays,
            );
        }
    }
    data.sky_light_mask = BitSet::from_u64(sky_light_mask);
    data.block_light_mask = BitSet::from_u64(block_light_mask);
    data.empty_sky_light_mask = BitSet::from_u64(empty_sky_light_mask);
    data.empty_block_light_mask = BitSet::from_u64(empty_block_light_mask);
    Ok(data)
}

/// Vanilla `prepareSectionData`: an all-dark section goes in the empty mask, any other is sent.
fn push_section(
    section: &LightContainer,
    bit: u64,
    mask: &mut u64,
    empty_mask: &mut u64,
    arrays: &mut Vec<Vec<u8>>,
) {
    match section {
        LightContainer::Empty(0) => *empty_mask |= bit,
        LightContainer::Empty(value) => {
            *mask |= bit;
            arrays.push(vec![*value << 4 | *value; LightContainer::ARRAY_SIZE]);
        }
        LightContainer::Full(data) => {
            *mask |= bit;
            arrays.push(data.to_vec());
        }
    }
}
