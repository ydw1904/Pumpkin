//! NBT Structure Template System
//!
//! This module provides functionality for loading and placing Minecraft structure
//! templates from `.nbt` files. This enables exact vanilla structure matching and
//! dramatically simplifies implementing structures like igloos, shipwrecks, villages, etc.
//!
//! # Architecture
//!
//! - [`StructureTemplate`]: Represents a loaded NBT template with size, palette, and blocks
//! - [`TemplatePiece`]: A structure piece that places blocks from a template
//! - [`Rotation`] and [`Mirror`](pumpkin_data::Mirror): Transform positions and block properties
//! - [`TemplateCache`]: Lazy-loading cache for embedded template files
//!
//! # Example Usage
//!
//! ```ignore
//! use pumpkin_world::generation::structure::template::{TemplateCache, TemplatePiece};
//! use pumpkin_data::Rotation;
//!
//! // Load a template from the cache
//! let template = TemplateCache::get("igloo/top").expect("Template not found");
//!
//! // Create a piece to place the template
//! let piece = TemplatePiece::new(template, rotation, mirror, position);
//! ```

mod block_state_resolver;
mod cache;
mod place_in_world;
pub mod processor;
mod structure_template;
mod template_piece;

use pumpkin_data::Block;
use pumpkin_data::BlockStateId;
use pumpkin_data::Rotation;
use pumpkin_data::fluid::Fluid;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::HeightMap;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{
    RandomDeriverImpl, RandomImpl, hash_block_pos, legacy_rand::LegacyRand,
};

use crate::generation::proto_chunk::StructureEntity;

pub use block_state_resolver::BlockStateResolver;
pub use cache::{
    TemplateCache, all_embedded_datapack_names, all_pool_names, all_structure_names,
    all_template_names, get_template, global_cache,
};
pub use place_in_world::TemplateBlockPlacement;
pub use processor::{
    StructureProcessor, clear_dynamic_processors, load_processor_list,
    register_dynamic_processor_list,
};
pub use pumpkin_data::{BlockState, Mirror as BlockMirror, Rotation as BlockRotation};
pub use structure_template::{
    JigsawBlockInfo, Palette, PaletteEntry, SimplePalette, StructureBlockInfo, StructureEntityInfo,
    StructurePlaceSettings, StructureTemplate, TemplateBlock, TemplateEntity,
};
pub use template_piece::TemplatePiece;

/// Abstraction over block placement, implemented by [`ProtoChunk`](crate::ProtoChunk) and
/// [`GenerationRegion`](crate::generation::update_shape::GenerationRegion) (worldgen) and
/// [`WorldBlockPlacer`] (live `/place template` command).
pub trait BlockPlacer {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId;
    /// Vanilla `LevelReader.getHeight`: the Y just above the heightmap's top block.
    fn get_height(&self, heightmap: HeightMap, x: i32, z: i32) -> i32;
    /// Sets a block like vanilla does with `UPDATE_KNOWN_SHAPE` (flag 16).
    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState);
    /// Sets a block like vanilla `WorldGenRegion.setBlock` without `UPDATE_KNOWN_SHAPE`, which
    /// also marks the block's post-processing position.
    fn set_block_state_with_post_processing(&mut self, pos: &Vector3<i32>, state: &BlockState);
    fn add_block_entity(&mut self, pos: &Vector3<i32>, nbt: NbtCompound);
    fn schedule_fluid_tick(&mut self, pos: &Vector3<i32>, fluid: &'static Fluid);
    fn schedule_block_tick(&mut self, pos: &Vector3<i32>, block: &'static Block);
}

/// Places a template at a world origin with an un-rotated XZ offset.
///
/// All rotation is handled internally:
/// - The offset is rotated to position the template correctly
/// - Block positions within the template are rotated
/// - Directional block properties (facing, axis, etc.) are rotated
/// - Block entities are created from template NBT data
///
/// `origin` is the base world position (x, y, z).
/// `offset` is the un-rotated XZ offset from origin (`x_offset`, `z_offset`) - rotation is applied automatically.
#[allow(clippy::too_many_arguments)]
pub fn place_template(
    placer: &mut impl BlockPlacer,
    template: &StructureTemplate,
    origin: Vector3<i32>,
    offset: (i32, i32),
    rotation: Rotation,
    skip_air: bool,
    apply_waterlogging: bool,
    processors: &[StructureProcessor],
    chunk_box: Option<&pumpkin_util::math::block_box::BlockBox>,
    world_seed: i64,
) {
    place_template_with_options(
        placer,
        template,
        origin,
        offset,
        rotation,
        skip_air,
        apply_waterlogging,
        processors,
        chunk_box,
        false,
        origin,
        world_seed,
    );
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn place_template_with_options(
    placer: &mut impl BlockPlacer,
    template: &StructureTemplate,
    origin: Vector3<i32>,
    offset: (i32, i32),
    rotation: Rotation,
    skip_air: bool,
    apply_waterlogging: bool,
    processors: &[StructureProcessor],
    chunk_box: Option<&pumpkin_util::math::block_box::BlockBox>,
    keep_jigsaws: bool,
    reference_pos: Vector3<i32>,
    world_seed: i64,
) {
    let (rotated_ox, rotated_oz) = rotation.rotate_offset(offset.0, offset.1);
    let world_x = origin.x + rotated_ox;
    let world_z = origin.z + rotated_oz;

    let mut context = processor::ProcessorContext::new(reference_pos);
    // Vanilla `processBlockInfos` only skips blocks outside the chunk when no processor needs
    // the whole piece.
    let process_only_in_chunk = !processors
        .iter()
        .any(StructureProcessor::evaluates_entire_piece_state);
    let mut processed = Vec::with_capacity(template.blocks.len());
    for block in &template.blocks {
        // Rotate block position within template bounds
        let local_pos = rotation.transform_pos(block.pos, template.size);

        let wx = world_x + local_pos.x;
        let wy = origin.y + local_pos.y;
        let wz = world_z + local_pos.z;

        if process_only_in_chunk
            && let Some(bbox) = chunk_box
            && (wx < bbox.min.x
                || wx > bbox.max.x
                || wy < bbox.min.y
                || wy > bbox.max.y
                || wz < bbox.min.z
                || wz > bbox.max.z)
        {
            continue;
        }

        let palette_entry = &template.palette[block.state as usize];

        // Structure blocks are data markers.
        if palette_entry.name == "minecraft:structure_block" {
            continue;
        }

        let mut block_entity_nbt = block.nbt.clone();
        let jigsaw_entry: PaletteEntry;
        let placed_entry = if !keep_jigsaws && palette_entry.name == "minecraft:jigsaw" {
            // Jigsaw blocks are replaced during template processing, before block entities are
            // collected. Keeping this in the placement pipeline avoids stale jigsaw entities.
            let final_state = block_entity_nbt
                .as_ref()
                .and_then(|nbt| nbt.get_string("final_state"))
                .unwrap_or("minecraft:air");
            jigsaw_entry = PaletteEntry::from_string(final_state);
            block_entity_nbt = None;
            &jigsaw_entry
        } else {
            palette_entry
        };

        // Structure void preserves the existing block, both from the palette itself and from a
        // jigsaw final state.
        if placed_entry.name == "minecraft:structure_void" {
            continue;
        }

        // Processors see the template's own states; vanilla rotates the result in `placeInWorld`.
        let Some(state) = BlockStateResolver::resolve_simple(placed_entry) else {
            continue;
        };

        let mut world_pos = Vector3::new(wx, wy, wz);

        let Some(state) = process_block(
            placer,
            processors,
            &mut context,
            block.pos.y,
            &mut world_pos,
            state,
            &mut block_entity_nbt,
        ) else {
            continue;
        };
        // Legacy pool elements ignore air after jigsaw replacement and custom processors.
        if skip_air && state.id.to_block_id() == pumpkin_data::Block::AIR.id {
            continue;
        }
        processed.push(ProcessedBlock {
            pos: world_pos,
            state,
            nbt: block_entity_nbt,
        });
    }

    // Vanilla's template position: transformed template positions are offsets from it, while
    // `transform_pos` keeps them inside the rotated size.
    let corner = rotation.rotate_offset(template.size.x - 1, template.size.z - 1);
    let position = Vector3::new(
        world_x - corner.0.min(0),
        origin.y,
        world_z - corner.1.min(0),
    );
    finalize_processing(
        placer,
        processors,
        &mut context,
        world_seed,
        position,
        &mut processed,
    );

    // Vanilla processes the whole template before placing anything, so rule processors read
    // the world as it was before this template. Jigsaw pieces place with a known shape
    // (`SinglePoolElement.getSettings`). The live commands leave other shape updates to the
    // world's own block updates.
    let mut placement = TemplateBlockPlacement::new(apply_waterlogging, true);
    for ProcessedBlock {
        pos: world_pos,
        state,
        nbt: block_entity_nbt,
    } in processed
    {
        let state = state.rotate(rotation);
        if chunk_box.is_some_and(|bbox| !bbox.contains_pos(&world_pos)) {
            continue;
        }
        placement.set_block(placer, world_pos, state);

        // Create block entities for interactive blocks (furnaces, chests, etc.)
        let final_block = pumpkin_data::Block::from_id(state.id.to_block_id());
        let block_entity_id = get_block_entity_id(final_block.name);
        if block_entity_nbt.is_some() || block_entity_id.is_some() {
            let fallback_id = block_entity_id.unwrap_or(final_block.name);
            let mut placed_nbt = NbtCompound::new();

            placed_nbt.put_string("id", fallback_id.to_string());
            placed_nbt.put_int("x", world_pos.x);
            placed_nbt.put_int("y", world_pos.y);
            placed_nbt.put_int("z", world_pos.z);

            if let Some(template_nbt) = &block_entity_nbt {
                for (key, value) in &template_nbt.child_tags {
                    if key.as_ref() != "x" && key.as_ref() != "y" && key.as_ref() != "z" {
                        placed_nbt.child_tags.insert(key.clone(), value.clone());
                    }
                }
            }

            if placed_nbt.get_string("LootTable").is_some()
                && placed_nbt.get_long("LootTableSeed").is_none()
            {
                let seed = hash_block_pos(world_pos.x, world_pos.y, world_pos.z);
                let mut random = LegacyRand::from_seed(seed as u64);
                placed_nbt.put_long("LootTableSeed", random.next_i64());
            }

            placer.add_block_entity(&world_pos, placed_nbt);
        }
    }

    placement.finish(placer);
}

/// Runs the processors on one template block, like the loop in vanilla
/// `StructureTemplate.processBlockInfos`. Every processor sees the position the previous ones
/// left and seeds its own random from it (`StructurePlaceSettings.getRandom(pos)`).
pub(crate) fn process_block(
    placer: &impl BlockPlacer,
    processors: &[StructureProcessor],
    context: &mut processor::ProcessorContext,
    template_y: i32,
    world_pos: &mut Vector3<i32>,
    mut state: &'static BlockState,
    nbt: &mut Option<NbtCompound>,
) -> Option<&'static BlockState> {
    for processor in processors {
        if let StructureProcessor::Gravity { heightmap, offset } = processor {
            // `GravityProcessor` keeps the block's height within the template.
            world_pos.y = placer.get_height((*heightmap).into(), world_pos.x, world_pos.z)
                + offset
                + template_y;
            continue;
        }
        let mut rng =
            LegacyRand::from_seed(hash_block_pos(world_pos.x, world_pos.y, world_pos.z) as u64);
        state =
            processor.process_with_context(placer, *world_pos, state, nbt, context, &mut rng)?;
    }
    Some(state)
}

/// A template block after the processors, like vanilla `StructureTemplate.StructureBlockInfo`.
pub(crate) struct ProcessedBlock {
    pub pos: Vector3<i32>,
    pub state: &'static BlockState,
    pub nbt: Option<NbtCompound>,
}

/// Vanilla `StructureProcessor.finalizeProcessing` for every processor, run once all blocks of
/// the piece went through [`process_block`]. `position` is the template's position.
pub(crate) fn finalize_processing(
    placer: &impl BlockPlacer,
    processors: &[StructureProcessor],
    context: &mut processor::ProcessorContext,
    world_seed: i64,
    position: Vector3<i32>,
    processed: &mut [ProcessedBlock],
) {
    for processor in processors {
        let StructureProcessor::Capped { limit, delegate } = processor else {
            continue;
        };
        // `CappedProcessor.finalizeProcessing`: replace up to `limit` randomly picked blocks.
        if limit.get_max() == 0 || processed.is_empty() {
            continue;
        }
        let mut random = LegacyRand::from_seed(world_seed as u64)
            .next_splitter()
            .split_pos(position.x, position.y, position.z);
        let max_to_replace = limit.get(&mut random).min(processed.len() as i32);
        if max_to_replace < 1 {
            continue;
        }
        // `Util.toShuffledList`
        let mut indices: Vec<usize> = (0..processed.len()).collect();
        for i in (2..=indices.len()).rev() {
            let swap_to = random.next_bounded_i32(i as i32) as usize;
            indices.swap(i - 1, swap_to);
        }
        let mut replaced = 0;
        for index in indices {
            if replaced >= max_to_replace {
                break;
            }
            let block = &mut processed[index];
            let mut nbt = block.nbt.clone();
            let mut rng =
                LegacyRand::from_seed(hash_block_pos(block.pos.x, block.pos.y, block.pos.z) as u64);
            if let Some(state) = delegate.process_with_context(
                placer,
                block.pos,
                block.state,
                &mut nbt,
                context,
                &mut rng,
            ) && (state.id != block.state.id || nbt != block.nbt)
            {
                replaced += 1;
                block.state = state;
                block.nbt = nbt;
            }
        }
    }
}

/// Vanilla `StructureTemplate.placeEntities`: hands each template entity inside `chunk_box` (all
/// of them without one) to `add_entity` with the saved data it is created from.
pub fn place_template_entities(
    mut add_entity: impl FnMut(StructureEntity),
    template: &StructureTemplate,
    origin: Vector3<i32>,
    rotation: Rotation,
    chunk_box: Option<&pumpkin_util::math::block_box::BlockBox>,
    finalize_entities: bool,
) {
    for entity in &template.entities {
        let block_pos = rotation.transform_pos(entity.block_pos, template.size);
        let world_block_pos = Vector3::new(
            origin.x + block_pos.x,
            origin.y + block_pos.y,
            origin.z + block_pos.z,
        );
        if chunk_box.is_some_and(|bbox| !bbox.contains_pos(&world_block_pos)) {
            continue;
        }

        let pos = match rotation {
            Rotation::None => entity.pos,
            Rotation::Clockwise90 => Vector3::new(
                f64::from(template.size.z) - entity.pos.z,
                entity.pos.y,
                entity.pos.x,
            ),
            Rotation::Rotate180 => Vector3::new(
                f64::from(template.size.x) - entity.pos.x,
                entity.pos.y,
                f64::from(template.size.z) - entity.pos.z,
            ),
            Rotation::CounterClockwise90 => Vector3::new(
                entity.pos.z,
                entity.pos.y,
                f64::from(template.size.x) - entity.pos.x,
            ),
        };
        let mut nbt = entity.nbt.clone();
        nbt.put(
            "Pos",
            NbtTag::List(vec![
                (f64::from(origin.x) + pos.x).into(),
                (f64::from(origin.y) + pos.y).into(),
                (f64::from(origin.z) + pos.z).into(),
            ]),
        );
        nbt.child_tags.remove("UUID");

        if let Some(rotation_nbt) = nbt.get_list("Rotation")
            && rotation_nbt.len() == 2
        {
            let yaw = rotation_nbt[0].extract_float().unwrap_or_default()
                + match rotation {
                    Rotation::None => 0.0,
                    Rotation::Clockwise90 => 90.0,
                    Rotation::Rotate180 => 180.0,
                    Rotation::CounterClockwise90 => 270.0,
                };
            let pitch = rotation_nbt[1].extract_float().unwrap_or_default();
            nbt.put("Rotation", NbtTag::List(vec![yaw.into(), pitch.into()]));
        }

        add_entity(StructureEntity {
            nbt,
            finalize: finalize_entities,
        });
    }
}

/// Block entity types implementing vanilla's `RandomizableContainer`, whose loot seed
/// `StructureTemplate.placeInWorld` draws from the piece random.
const RANDOMIZABLE_CONTAINERS: &[&str] = &[
    "chest",
    "trapped_chest",
    "barrel",
    "dispenser",
    "dropper",
    "hopper",
    "shulker_box",
    "crafter",
    "decorated_pot",
];

/// Whether the state's block entity is a vanilla `RandomizableContainer`.
pub(crate) fn is_randomizable_container(state: &BlockState) -> bool {
    pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
        .get(usize::from(state.block_entity_type))
        .is_some_and(|name| RANDOMIZABLE_CONTAINERS.contains(name))
}

/// Returns the block entity ID for blocks that require one, or None if not needed.
pub(crate) fn get_block_entity_id(block_name: &str) -> Option<&'static str> {
    let name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    match name {
        "furnace" => Some("minecraft:furnace"),
        "chest" => Some("minecraft:chest"),
        "trapped_chest" => Some("minecraft:trapped_chest"),
        "barrel" => Some("minecraft:barrel"),
        "hopper" => Some("minecraft:hopper"),
        "dropper" => Some("minecraft:dropper"),
        "dispenser" => Some("minecraft:dispenser"),
        "brewing_stand" => Some("minecraft:brewing_stand"),
        "blast_furnace" => Some("minecraft:blast_furnace"),
        "smoker" => Some("minecraft:smoker"),
        "shulker_box" => Some("minecraft:shulker_box"),
        "bed" => Some("minecraft:bed"),
        "suspicious_sand" | "suspicious_gravel" => Some("minecraft:brushable_block"),
        "decorated_pot" => Some("minecraft:decorated_pot"),
        "spawner" => Some("minecraft:mob_spawner"),
        "trial_spawner" => Some("minecraft:trial_spawner"),
        "vault" => Some("minecraft:vault"),
        "crafter" => Some("minecraft:crafter"),
        "creaking_heart" => Some("minecraft:creaking_heart"),
        "chiseled_bookshelf" => Some("minecraft:chiseled_bookshelf"),
        "beehive" | "bee_nest" => Some("minecraft:beehive"),
        "campfire" | "soul_campfire" => Some("minecraft:campfire"),
        "sculk_sensor" | "calibrated_sculk_sensor" => Some("minecraft:sculk_sensor"),
        "sculk_catalyst" => Some("minecraft:sculk_catalyst"),
        "sculk_shrieker" => Some("minecraft:sculk_shrieker"),
        "dragon_head"
        | "dragon_wall_head"
        | "skeleton_skull"
        | "skeleton_wall_skull"
        | "wither_skeleton_skull"
        | "wither_skeleton_wall_skull"
        | "zombie_head"
        | "zombie_wall_head"
        | "player_head"
        | "player_wall_head"
        | "creeper_head"
        | "creeper_wall_head"
        | "piglin_head"
        | "piglin_wall_head" => Some("minecraft:skull"),
        "sign" | "oak_sign" | "spruce_sign" | "birch_sign" | "jungle_sign" | "acacia_sign"
        | "dark_oak_sign" | "mangrove_sign" | "cherry_sign" | "bamboo_sign" | "crimson_sign"
        | "warped_sign" | "pale_oak_sign" => Some("minecraft:sign"),
        "hanging_sign"
        | "oak_hanging_sign"
        | "spruce_hanging_sign"
        | "birch_hanging_sign"
        | "jungle_hanging_sign"
        | "acacia_hanging_sign"
        | "dark_oak_hanging_sign"
        | "mangrove_hanging_sign"
        | "cherry_hanging_sign"
        | "bamboo_hanging_sign"
        | "crimson_hanging_sign"
        | "warped_hanging_sign"
        | "pale_oak_hanging_sign" => Some("minecraft:hanging_sign"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::Block;

    struct CollectingPlacer(Vec<BlockStateId>);

    impl BlockPlacer for CollectingPlacer {
        fn get_block_state(&self, _pos: &Vector3<i32>) -> BlockStateId {
            Block::AIR.default_state.id
        }

        fn get_height(&self, _heightmap: HeightMap, _x: i32, _z: i32) -> i32 {
            0
        }

        fn set_block_state(&mut self, _pos: &Vector3<i32>, state: &BlockState) {
            self.0.push(state.id);
        }

        fn set_block_state_with_post_processing(&mut self, pos: &Vector3<i32>, state: &BlockState) {
            self.set_block_state(pos, state);
        }

        fn add_block_entity(&mut self, _pos: &Vector3<i32>, _nbt: NbtCompound) {}

        fn schedule_fluid_tick(&mut self, _pos: &Vector3<i32>, _fluid: &'static Fluid) {}

        fn schedule_block_tick(&mut self, _pos: &Vector3<i32>, _block: &'static Block) {}
    }

    #[test]
    fn structure_void_is_never_placed() {
        let mut placed_templates = 0;

        for name in all_template_names() {
            let Some(template) = get_template(name) else {
                continue;
            };

            let mut placer = CollectingPlacer(Vec::new());
            place_template(
                &mut placer,
                &template,
                Vector3::new(0, 0, 0),
                (0, 0),
                Rotation::None,
                false,
                false,
                &[],
                None,
                0,
            );

            for state_id in &placer.0 {
                assert_ne!(
                    state_id.to_block_id(),
                    Block::STRUCTURE_VOID.id,
                    "{name} placed a structure void"
                );
            }

            placed_templates += 1;
        }

        assert!(placed_templates > 0);
    }
}
