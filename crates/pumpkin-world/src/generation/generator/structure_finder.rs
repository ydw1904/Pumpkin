use pumpkin_data::structures::{
    ConcentricRingsStructurePlacement, RandomSpreadStructurePlacement, StructurePlacement,
    StructurePlacementType, StructureSet,
};
use pumpkin_util::math::{floor_div, position::BlockPos};

use crate::generation::{
    proto_chunk::ProtoChunk,
    structure::placement::{GlobalStructureCache, StrongholdLayout, get_structure_chunk_in_region},
};

use super::WorldGenerator;

/// Block-level position of a found structure plus squared distance from the
/// search origin, used internally to track the running nearest candidate.
#[derive(Debug, Clone)]
pub struct FoundStructure {
    pub pos: BlockPos,
    pub distance_sq: f64,
}

/// Vanilla `StructurePlacement.getLocatePos`: the chunk's minimum block corner moved by the
/// placement's `locate_offset`.
#[must_use]
pub const fn get_locate_pos(
    placement: &StructurePlacement,
    chunk_x: i32,
    chunk_z: i32,
) -> BlockPos {
    let (offset_x, offset_y, offset_z) = placement.locate_offset;
    BlockPos::new(
        (chunk_x << 4) + offset_x,
        offset_y,
        (chunk_z << 4) + offset_z,
    )
}

/// Finds the block position of the nearest structure whose placement is listed
/// in `placements`, within `max_search_radius` chunk-region rings.
///
/// Mirrors the two-pass logic in vanilla's
/// `ChunkGenerator.findNearestMapStructure`:
///
/// 1. **Concentric-rings** placements (strongholds) are resolved in one pass
///    from the pre-computed [`GlobalStructureCache`].
/// 2. **Random-spread** placements are searched ring-by-ring outward, stopping
///    at the first radius that produces any result.
///
/// The best candidate from both passes is returned.
///
/// Returns [`StructureSearch::Pending`] while the stronghold positions are still
/// being computed, so the caller can retry instead of reporting a wrong result.
pub fn find_nearest_structure(
    origin: BlockPos,
    placements: &[&StructurePlacement],
    max_search_radius: i32,
    world_seed: i64,
    global_cache: &GlobalStructureCache,
) -> StructureSearch {
    if placements.is_empty() {
        return StructureSearch::NotFound;
    }

    let mut nearest: Option<FoundStructure> = None;

    // ── Pass 1: Concentric-rings (strongholds) ──────────────────────────────
    for p in placements {
        if let StructurePlacementType::ConcentricRings(rings) = &p.placement_type {
            match find_nearest_concentric(origin, p, rings, global_cache) {
                ConcentricSearch::Pending => return StructureSearch::Pending,
                ConcentricSearch::Done(Some(found))
                    if nearest
                        .as_ref()
                        .is_none_or(|n| found.distance_sq < n.distance_sq) =>
                {
                    nearest = Some(found);
                }
                ConcentricSearch::Done(_) => {}
            }
        }
    }

    let random_spread: Vec<(&StructurePlacement, &RandomSpreadStructurePlacement)> = placements
        .iter()
        .filter_map(|p| {
            if let StructurePlacementType::RandomSpread(r) = &p.placement_type {
                Some((*p, r))
            } else {
                None
            }
        })
        .collect();

    if !random_spread.is_empty() {
        let chunk_origin_x = origin.0.x >> 4;
        let chunk_origin_z = origin.0.z >> 4;

        'radius: for radius in 0..=max_search_radius {
            for (placement, spread) in &random_spread {
                if let Some(found) = find_nearest_random_spread_at_radius(
                    origin,
                    chunk_origin_x,
                    chunk_origin_z,
                    radius,
                    world_seed,
                    placement,
                    spread,
                ) {
                    if nearest
                        .as_ref()
                        .is_none_or(|n| found.distance_sq < n.distance_sq)
                    {
                        nearest = Some(found);
                    }
                    break 'radius;
                }
            }
        }
    }

    nearest.map_or(StructureSearch::NotFound, |f| StructureSearch::Found(f.pos))
}

/// Result of [`find_nearest_structure`].
#[derive(Debug, Clone)]
pub enum StructureSearch {
    Found(BlockPos),
    NotFound,
    Pending,
}

/// Finds a start of one of `target_structures` in `structure_set`, searching ring by ring
/// outward like vanilla `ChunkGenerator.findNearestMapStructure` does for one placement.
///
/// Returns the position along with which of the targets was found.
#[must_use]
pub fn find_nearest_structure_start(
    origin: BlockPos,
    structure_set: &StructureSet,
    target_structures: &[pumpkin_data::structures::StructureKeys],
    max_search_radius: i32,
    generator: &WorldGenerator,
) -> Option<(BlockPos, pumpkin_data::structures::StructureKeys)> {
    (0..=max_search_radius).find_map(|radius| {
        find_structure_start_in_ring(
            origin,
            structure_set,
            target_structures,
            radius,
            generator,
            &mut |_, _, _| true,
        )
    })
}

/// Vanilla `ChunkGenerator.findNearestMapStructure` over several random-spread sets: each ring
/// asks every set for its first start and keeps the nearest before moving outward.
///
/// `try_reference` is vanilla's `createReference` check: it gets each start found, by its
/// structure and chunk, and a start it turns down is skipped.
#[must_use]
pub fn find_nearest_structure_start_in_sets(
    origin: BlockPos,
    searches: &[(&StructureSet, &[pumpkin_data::structures::StructureKeys])],
    max_search_radius: i32,
    generator: &WorldGenerator,
    try_reference: &mut dyn FnMut(pumpkin_data::structures::StructureKeys, i32, i32) -> bool,
) -> Option<(BlockPos, pumpkin_data::structures::StructureKeys)> {
    (0..=max_search_radius).find_map(|radius| {
        searches
            .iter()
            .filter_map(|&(structure_set, target_structures)| {
                find_structure_start_in_ring(
                    origin,
                    structure_set,
                    target_structures,
                    radius,
                    generator,
                    try_reference,
                )
            })
            .min_by_key(|(pos, _)| {
                let dx = i64::from(pos.0.x - origin.0.x);
                let dz = i64::from(pos.0.z - origin.0.z);
                dx * dx + dz * dz
            })
    })
}

/// One ring of vanilla `ChunkGenerator.getNearestGeneratedStructure` for a random-spread set.
///
/// Returns the first candidate in vanilla's scan order whose chunk really starts one of
/// `target_structures` that `try_reference` accepts, and which one. Like vanilla, that is not
/// always the nearest candidate of the ring.
#[must_use]
pub fn find_structure_start_in_ring(
    origin: BlockPos,
    structure_set: &StructureSet,
    target_structures: &[pumpkin_data::structures::StructureKeys],
    radius: i32,
    generator: &WorldGenerator,
    try_reference: &mut dyn FnMut(pumpkin_data::structures::StructureKeys, i32, i32) -> bool,
) -> Option<(BlockPos, pumpkin_data::structures::StructureKeys)> {
    use crate::{
        biome::BiomeSupplier,
        generation::{
            noise::router::multi_noise_sampler::MultiNoiseSampler,
            structure::{
                height_sampler::NoiseHeightSampler,
                lazily_generate_structure,
                placement::should_generate_structure,
                structures::{StructureGeneratorContext, create_chunk_random},
            },
        },
    };
    use pumpkin_data::structures::Structure;

    let WorldGenerator::Noise(noise_generator) = generator else {
        return None;
    };
    let StructurePlacementType::RandomSpread(placement) = &structure_set.placement.placement_type
    else {
        return None;
    };

    let region_origin_x = floor_div(origin.0.x >> 4, placement.spacing);
    let region_origin_z = floor_div(origin.0.z >> 4, placement.spacing);
    let world_seed = noise_generator.random_config.seed as i64;
    let global_cache = &noise_generator.global_structure_cache;

    let start_for = |key: pumpkin_data::structures::StructureKeys, chunk_x: i32, chunk_z: i32| {
        global_cache.get_or_compute_structure_start(key, chunk_x, chunk_z, || {
            let settings = noise_generator.settings;
            // The start is cached and chunk generation reuses it, so it has to be
            // computed the same way: real column heights and this dimension's biomes.
            let mut height_sampler = NoiseHeightSampler::new(noise_generator);
            let mut biome_sampler =
                MultiNoiseSampler::generate(&noise_generator.base_router.multi_noise);
            let biome_supplier: &dyn BiomeSupplier = &noise_generator.biome_supplier;
            let context = StructureGeneratorContext {
                seed: world_seed,
                chunk_x,
                chunk_z,
                random: create_chunk_random(world_seed, chunk_x, chunk_z),
                sea_level: settings.sea_level,
                min_y: (settings.shape.min_y as i32).max(noise_generator.dimension.min_y),
                height: settings
                    .shape
                    .height
                    .min(noise_generator.dimension.height as u16),
                height_sampler: Some(&mut height_sampler),
                structure_key: Some(key),
            };
            lazily_generate_structure(
                &key,
                Structure::get(&key),
                context,
                biome_supplier,
                &mut biome_sampler,
            )
        })
    };

    for region_x_offset in -radius..=radius {
        for region_z_offset in -radius..=radius {
            if region_x_offset.abs() != radius && region_z_offset.abs() != radius {
                continue;
            }
            let (chunk_x, chunk_z) = get_structure_chunk_in_region(
                placement,
                world_seed,
                region_origin_x + region_x_offset,
                region_origin_z + region_z_offset,
                structure_set.placement.salt,
            );
            if !should_generate_structure(
                &structure_set.placement,
                &noise_generator.structure_calculator,
                chunk_x,
                chunk_z,
                global_cache,
            ) {
                continue;
            }

            // The chunk only holds the first structure of the set, in its weighted order, that
            // can start there (vanilla `ChunkGenerator.createStructures`), so a target only
            // counts when it is that structure.
            let Some(key) = ProtoChunk::weighted_structure_order(
                structure_set.structures,
                noise_generator.random_config.seed,
                chunk_x,
                chunk_z,
            )
            .into_iter()
            .map(|entry| entry.structure)
            .find(|&key| start_for(key, chunk_x, chunk_z).is_some()) else {
                continue;
            };
            if target_structures.contains(&key) && try_reference(key, chunk_x, chunk_z) {
                return Some((
                    get_locate_pos(&structure_set.placement, chunk_x, chunk_z),
                    key,
                ));
            }
        }
    }
    None
}

fn find_nearest_concentric(
    origin: BlockPos,
    placement: &StructurePlacement,
    // Kept for potential future bounds / distance validation.
    _rings: &ConcentricRingsStructurePlacement,
    global_cache: &GlobalStructureCache,
) -> ConcentricSearch {
    // No block, callers run in server tasks and the tick, which shutdown waits on
    let strongholds = match global_cache.try_get_stronghold_chunks() {
        StrongholdLayout::Ready(strongholds) => strongholds,
        StrongholdLayout::Pending => return ConcentricSearch::Pending,
        StrongholdLayout::Failed => return ConcentricSearch::Done(None),
    };

    let ox = origin.0.x as f64;
    let oz = origin.0.z as f64;

    strongholds
        .iter()
        .map(|(cx, cz)| {
            let pos = get_locate_pos(placement, *cx, *cz);
            let dx = f64::from(pos.0.x) - ox;
            let dz = f64::from(pos.0.z) - oz;
            FoundStructure {
                pos,
                distance_sq: dx * dx + dz * dz,
            }
        })
        .min_by(|a, b| a.distance_sq.total_cmp(&b.distance_sq))
        .map_or(ConcentricSearch::Done(None), |found| {
            ConcentricSearch::Done(Some(found))
        })
}

/// Outcome of a stronghold lookup: the ring positions may still be computing.
enum ConcentricSearch {
    Pending,
    Done(Option<FoundStructure>),
}

fn find_nearest_random_spread_at_radius(
    origin: BlockPos,
    chunk_origin_x: i32,
    chunk_origin_z: i32,
    radius: i32,
    world_seed: i64,
    placement: &StructurePlacement,
    spread: &RandomSpreadStructurePlacement,
) -> Option<FoundStructure> {
    let spacing = spread.spacing;
    let ox = origin.0.x as f64;
    let oz = origin.0.z as f64;

    let mut best: Option<FoundStructure> = None;

    for rx_off in -radius..=radius {
        for rz_off in -radius..=radius {
            if rx_off.abs() != radius && rz_off.abs() != radius {
                continue;
            }

            let rx = floor_div(chunk_origin_x, spacing) + rx_off;
            let rz = floor_div(chunk_origin_z, spacing) + rz_off;

            let (struct_cx, struct_cz) =
                get_structure_chunk_in_region(spread, world_seed, rx, rz, placement.salt);

            let pos = get_locate_pos(placement, struct_cx, struct_cz);
            let dx = f64::from(pos.0.x) - ox;
            let dz = f64::from(pos.0.z) - oz;
            let dist_sq = dx * dx + dz * dz;

            if best.as_ref().is_none_or(|b| dist_sq < b.distance_sq) {
                best = Some(FoundStructure {
                    pos,
                    distance_sq: dist_sq,
                });
            }
        }
    }

    best
}
