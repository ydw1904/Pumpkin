use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::position::BlockPos;

pub mod armor_stand;
pub mod cushion;
pub mod display;
pub mod end_crystal;
pub mod item_frame;
pub mod leash_knot;
pub mod painting;
pub mod transformation;

/// Saves the block a hanging entity is attached in, like vanilla's `BlockAttachedEntity`.
/// Vanilla discards a hanging entity loaded without it.
fn write_block_pos(nbt: &mut NbtCompound, pos: BlockPos) {
    nbt.put(
        "block_pos",
        NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
    );
}
