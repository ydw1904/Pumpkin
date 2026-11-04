use std::sync::{Mutex, PoisonError};

use pumpkin_data::structures::StructureKeys;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use rustc_hash::FxHashMap;

/// Vanilla `StructureStart.getMaxReferences`.
const MAX_REFERENCES: i32 = 1;
/// Most starts read back from the file, so a damaged one can't allocate without bound.
const MAX_SAVED_STARTS: usize = 1 << 20;

/// The saved data file the counts are kept in, under `data/pumpkin/`.
pub const SAVED_DATA_PATH: &str = "structure_references";

type StartKey = (String, StructureKeys, i32, i32);

/// Vanilla `StructureStart.references`: how many exploration maps lead to each structure start,
/// so that maps skip the starts earlier ones claimed.
///
/// Vanilla saves the count with the start in its chunk. Pumpkin doesn't keep starts in chunks,
/// so the counts live in their own file, keyed by dimension, structure and start chunk.
#[derive(Default)]
pub struct StructureReferences {
    counts: Mutex<FxHashMap<StartKey, i32>>,
}

impl StructureReferences {
    #[must_use]
    pub fn load(data: Option<&NbtCompound>) -> Self {
        let mut counts = FxHashMap::default();
        if let Some(starts) = data.and_then(|data| data.get_list("starts")) {
            for start in starts.iter().take(MAX_SAVED_STARTS) {
                let NbtTag::Compound(start) = start else {
                    continue;
                };
                let (Some(dimension), Some(structure), Some(x), Some(z), Some(references)) = (
                    start.get_string("dimension"),
                    start.get_string("id").and_then(|id| {
                        StructureKeys::from_name(id.strip_prefix("minecraft:").unwrap_or(id))
                    }),
                    start.get_int("ChunkX"),
                    start.get_int("ChunkZ"),
                    start.get_int("references"),
                ) else {
                    continue;
                };
                counts.insert((dimension.to_string(), structure, x, z), references);
            }
        }
        Self {
            counts: Mutex::new(counts),
        }
    }

    #[must_use]
    pub fn to_saved_data(&self) -> NbtCompound {
        let counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let starts = counts
            .iter()
            .map(|((dimension, structure, x, z), references)| {
                let mut start = NbtCompound::new();
                start.put_string("dimension", dimension.clone());
                start.put_string("id", format!("minecraft:{}", structure.to_name()));
                start.put_int("ChunkX", *x);
                start.put_int("ChunkZ", *z);
                start.put_int("references", *references);
                NbtTag::Compound(start)
            })
            .collect();
        let mut data = NbtCompound::new();
        data.put_list("starts", starts);
        data
    }

    /// Vanilla `ChunkGenerator.tryAddReference`: adds a reference to the start unless it
    /// already has as many as `StructureStart.canBeReferenced` allows.
    pub fn try_add_reference(
        &self,
        dimension: &str,
        structure: StructureKeys,
        chunk_x: i32,
        chunk_z: i32,
    ) -> bool {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let references = counts
            .entry((dimension.to_string(), structure, chunk_x, chunk_z))
            .or_insert(0);
        if *references >= MAX_REFERENCES {
            return false;
        }
        *references += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A start claimed by one map is skipped by the next, also after a save and load.
    #[test]
    fn a_referenced_start_stays_claimed_across_saves() {
        let references = StructureReferences::default();
        let overworld = "minecraft:overworld";
        assert!(references.try_add_reference(overworld, StructureKeys::BuriedTreasure, 3, -7));
        assert!(!references.try_add_reference(overworld, StructureKeys::BuriedTreasure, 3, -7));
        assert!(references.try_add_reference(overworld, StructureKeys::BuriedTreasure, 3, -6));

        let loaded = StructureReferences::load(Some(&references.to_saved_data()));
        assert!(!loaded.try_add_reference(overworld, StructureKeys::BuriedTreasure, 3, -7));
        assert!(loaded.try_add_reference(
            "minecraft:the_nether",
            StructureKeys::BuriedTreasure,
            3,
            -7
        ));
    }
}
