#![allow(clippy::print_stdout, clippy::needless_pass_by_value)]

use crate::biome::{BiomeSupplier, MultiNoiseBiomeSupplier};
use crate::generation::noise::router::multi_noise_sampler::MultiNoiseSampler;
use crate::generation::{generator::WorldGenerator, get_world_gen, proto_chunk::ProtoChunk};
use pumpkin_data::chunk::Biome;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::world_seed::Seed;

#[derive(serde::Deserialize)]
struct BiomeHashesJson {
    seed: u64,
    overworld: DimensionBiomeHashes,
    the_nether: DimensionBiomeHashes,
    the_end: DimensionBiomeHashes,
}

#[derive(serde::Deserialize)]
struct DimensionBiomeHashes {
    min_section_y: i32,
    max_section_y: i32,
    hashes: Vec<(i32, i32, String)>,
}

// The dump was made one chunk after another, and each biome search starts from the leaf the
// previous one found (vanilla `Climate.RTree.lastResult`), which wins exact distance ties. One
// sampler carried through the chunks in order reproduces that.
fn verify_biome_hashes_for_dimension(dim: Dimension, data: &DimensionBiomeHashes, seed: u64) {
    let world_gen = get_world_gen(Seed(seed), dim, false, Vec::new(), String::new());
    let WorldGenerator::Noise(generator) = &*world_gen else {
        unreachable!()
    };

    let mut sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);
    for (cx, cz, expected_hash) in &data.hashes {
        let mut chunk = ProtoChunk::new(*cx, *cz, &world_gen);
        chunk.populate_biomes(generator, &mut sampler);
        let actual_hash =
            compute_proto_chunk_biome_hash(&chunk, data.min_section_y, data.max_section_y);
        assert_eq!(
            &actual_hash, expected_hash,
            "Biome hash mismatch at chunk ({cx}, {cz})"
        );
    }
}

#[test]
fn nether_biome_hashes_match_vanilla() {
    let json: BiomeHashesJson =
        pumpkin_util::read_data_from_file!("../../../../../assets/tests/biome_hashes.json");
    verify_biome_hashes_for_dimension(Dimension::THE_NETHER, &json.the_nether, json.seed);
}

// This chunk's temperature sits exactly halfway between crimson_forest and nether_wastes, so
// its biomes depend on where the tree search starts. Biomes sampled earlier on the same thread
// must not change them.
#[test]
fn nether_chunk_biomes_ignore_earlier_samples() {
    let json: BiomeHashesJson =
        pumpkin_util::read_data_from_file!("../../../../../assets/tests/biome_hashes.json");
    let data = &json.the_nether;
    let (cx, cz) = (121_290, -208_809);
    let world_gen = get_world_gen(
        Seed(json.seed),
        Dimension::THE_NETHER,
        false,
        Vec::new(),
        String::new(),
    );
    let WorldGenerator::Noise(generator) = &*world_gen else {
        unreachable!()
    };
    let generate = || {
        let mut chunk = ProtoChunk::new(cx, cz, &world_gen);
        chunk.step_to_biomes(generator);
        compute_proto_chunk_biome_hash(&chunk, data.min_section_y, data.max_section_y)
    };

    let first = generate();
    let found_crimson = (0..64 * 64).any(|i| {
        let mut sampler = MultiNoiseSampler::generate(&generator.base_router.multi_noise);
        let (x, z) = (cx * 4 + i % 64 - 32, cz * 4 + i / 64 - 32);
        MultiNoiseBiomeSupplier::NETHER.biome(x, 0, z, &mut sampler) == &Biome::CRIMSON_FOREST
    });
    assert!(found_crimson, "no crimson_forest sample near the chunk");
    assert_eq!(generate(), first);
}

#[test]
fn end_biome_hashes_match_vanilla() {
    let json: BiomeHashesJson =
        pumpkin_util::read_data_from_file!("../../../../../assets/tests/biome_hashes.json");
    verify_biome_hashes_for_dimension(Dimension::THE_END, &json.the_end, json.seed);
}

#[test]
fn overworld_biome_hashes_match_vanilla() {
    let json: BiomeHashesJson =
        pumpkin_util::read_data_from_file!("../../../../../assets/tests/biome_hashes.json");
    verify_biome_hashes_for_dimension(Dimension::OVERWORLD, &json.overworld, json.seed);
}

fn compute_proto_chunk_biome_hash(
    chunk: &ProtoChunk,
    min_section_y: i32,
    max_section_y: i32,
) -> String {
    let mut ctx = md5::Context::new();
    for section_y in min_section_y..=max_section_y {
        ctx.consume([section_y as u8]);
        for y in 0..4i32 {
            for z in 0..4i32 {
                for x in 0..4i32 {
                    let biome = chunk.get_biome(x, section_y * 4 + y, z);
                    let name = biome
                        .registry_id
                        .strip_prefix("minecraft:")
                        .unwrap_or(biome.registry_id);
                    ctx.consume(name.as_bytes());
                }
            }
        }
    }
    format!("{:x}", ctx.finalize())
}
