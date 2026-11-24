#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk_map::ChunkMap;

const CHUNKS: i32 = 12;

fn read(map: &ChunkMap, pos: BlockPos) -> Option<BlockStateId> {
    let (chunk, relative) = pos.chunk_and_chunk_relative_position();
    map.read(&chunk, |chunk| {
        chunk
            .section
            .get_block_absolute_y(relative.x as usize, relative.y, relative.z as usize)
    })?
}

/// 12x12 chunks with a stone floor and two mixed layers of hoppers, furnaces, pistons and
/// observers, so the read sections use an indexed palette.
fn filled_map() -> ChunkMap {
    let map = ChunkMap::default();
    let blocks = [
        Block::HOPPER.default_state.id,
        Block::FURNACE.default_state.id,
        Block::PISTON.default_state.id,
        Block::OBSERVER.default_state.id,
        Block::STONE.default_state.id,
    ];
    for cx in 0..CHUNKS {
        for cz in 0..CHUNKS {
            let chunk = ChunkData::empty_sync(cx, cz);
            for x in 0..16 {
                for z in 0..16 {
                    for y in 0..64 {
                        chunk.set_block_absolute_y(x, y, z, Block::STONE.default_state.id);
                    }
                    let state = blocks[(x * 7 + z * 3 + (cx * 5 + cz) as usize) % blocks.len()];
                    chunk.set_block_absolute_y(x, 150, z, state);
                    chunk.set_block_absolute_y(x, 151, z, Block::HOPPER.default_state.id);
                }
            }
            map.insert(Vector2::new(cx, cz), chunk);
        }
    }
    map
}

fn scattered() -> Vec<BlockPos> {
    (0..4096u32)
        .map(|i| {
            let h = i.wrapping_mul(2_654_435_761);
            let width = (CHUNKS * 16) as u32;
            BlockPos::new(
                (h % width) as i32,
                140 + ((h >> 8) % 20) as i32,
                ((h >> 16) % width) as i32,
            )
        })
        .collect()
}

fn bench_reads(c: &mut Criterion, name: &str, map: &ChunkMap) {
    let positions = scattered();
    let mut group = c.benchmark_group(name);
    group.bench_function("same chunk", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            black_box(read(map, BlockPos::new((i & 15) as i32, 150, 5)))
        });
    });
    group.bench_function("neighbours", |b| {
        // A position and its six neighbours, like a neighbour update.
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            let pos = positions[i & 4095];
            let mut acc = read(map, pos);
            for offset in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                acc = acc.or(read(map, pos.offset(offset.into())));
            }
            black_box(acc)
        });
    });
    group.bench_function("scattered", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            black_box(read(map, positions[i & 4095]))
        });
    });
    group.bench_function("unloaded", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            black_box(read(
                map,
                BlockPos::new(-100 - (i & 63) as i32 * 16, 150, 5),
            ))
        });
    });
    group.finish();
}

fn block_read(c: &mut Criterion) {
    let map = filled_map();
    bench_reads(c, "block read", &map);

    // Other threads reading the same chunks, like parallel entity islands and block entities.
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for t in 0..7u32 {
            let (map, stop) = (&map, &stop);
            scope.spawn(move || {
                let positions = scattered();
                let mut i = t as usize * 512;
                while !stop.load(Ordering::Relaxed) {
                    i = i.wrapping_add(1);
                    black_box(read(map, positions[i & 4095]));
                }
            });
        }
        bench_reads(c, "block read with 7 readers", &map);
        stop.store(true, Ordering::Relaxed);
    });

    // One other thread reading the chunk the measured reads go to. Every read takes the
    // section lock, so its cache line moves between the two cores on each read.
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (map, stop) = (&map, &stop);
        scope.spawn(move || {
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                i = i.wrapping_add(1);
                black_box(read(map, BlockPos::new((i & 15) as i32, 150, 9)));
            }
        });
        bench_reads(c, "block read with a reader in the same chunk", map);
        stop.store(true, Ordering::Relaxed);
    });
}

criterion_group!(benches, block_read);
criterion_main!(benches);
