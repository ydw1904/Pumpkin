use crate::entity::player::Player;
use dashmap::DashMap;
use pumpkin_data::dimension::Dimension;
use pumpkin_inventory::crafting::recipe_provider::MAX_MAP_SCALE;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use pumpkin_world::world_info::data_files::{read_saved_data, write_saved_data};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::error;

/// `MapItemSavedData.MAP_SIZE`.
const MAP_SIZE: usize = 128;

/// How often, in ticks, the player icon picks a new random rotation in the Nether.
const NETHER_SPIN_INTERVAL_TICKS: i64 = 10;
/// Vanilla's arbitrary constants for the hash that picks the Nether icon rotation.
const NETHER_SPIN_HASH_MULTIPLIER: i32 = 34_187_121;
const NETHER_SPIN_HASH_INCREMENT: i32 = 121;

/// `MapItemSavedData.createFresh`: the center of the map at this scale that covers the origin.
#[must_use]
pub fn fresh_map_center(origin_x: i32, origin_z: i32, scale: i8) -> (i32, i32) {
    let size = 128 * (1 << scale);
    let center = |origin: i32| {
        (f64::from(origin + 64) / f64::from(size)).floor() as i32 * size + size / 2 - 64
    };
    (center(origin_x), center(origin_z))
}

/// Vanilla keeps maps in the overworld's `SavedDataStorage` as `data/minecraft/maps/<id>.dat`.
pub struct MapManager {
    world_path: PathBuf,
    /// Maps read so far; `None` remembers that a map has no file, like vanilla's cache.
    maps: DashMap<i32, Option<Arc<Mutex<MapData>>>>,
}

impl MapManager {
    #[must_use]
    pub fn new(world_path: PathBuf) -> Self {
        Self {
            world_path,
            maps: DashMap::new(),
        }
    }

    /// `ServerLevel.getMapData`: reads the map from disk on first use.
    #[must_use]
    pub fn get_map(&self, id: i32) -> Option<Arc<Mutex<MapData>>> {
        self.maps
            .entry(id)
            .or_insert_with(|| {
                read_saved_data(&self.world_path, "minecraft", &map_key(id))
                    .and_then(|data| MapData::from_nbt(&data))
                    .map(|data| Arc::new(Mutex::new(data)))
            })
            .clone()
    }

    pub fn insert_map(&self, id: i32, data: MapData) {
        self.maps.insert(id, Some(Arc::new(Mutex::new(data))));
    }

    #[must_use]
    pub fn create_map(
        &self,
        id: i32,
        dimension: Dimension,
        x: i32,
        z: i32,
        scale: i8,
    ) -> Arc<Mutex<MapData>> {
        let map = Arc::new(Mutex::new(MapData::new(dimension, x, z, scale)));
        self.maps.insert(id, Some(map.clone()));
        map
    }

    /// `SavedDataStorage.scheduleSave` for maps: writes every map changed since the last save.
    pub fn save(&self, data_version: i32) {
        for entry in &self.maps {
            let Some(map) = entry.value() else {
                continue;
            };
            let data = {
                let mut map = map
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !map.needs_save {
                    continue;
                }
                map.needs_save = false;
                map.to_nbt()
            };
            let id = *entry.key();
            if let Err(err) = write_saved_data(
                &self.world_path,
                "minecraft",
                &map_key(id),
                data,
                data_version,
            ) {
                error!("Failed to save map {id}: {err}");
            }
        }
    }
}

/// `MapId.key`.
fn map_key(id: i32) -> String {
    format!("maps/{id}")
}

pub struct MapData {
    pub scale: i8,
    pub locked: bool,
    pub dimension: Dimension,
    pub center_x: i32,
    pub center_z: i32,
    pub tracking_position: bool,
    pub unlimited_tracking: bool,
    pub colors: Box<[u8; MAP_SIZE * MAP_SIZE]>,
    pub decorations: Vec<MapDecoration>,
    /// Banner and frame markers, kept as saved so vanilla gets them back.
    banners: Vec<NbtTag>,
    frames: Vec<NbtTag>,
    pub dirty: bool,
    /// `SavedData.isDirty`: the map has changed since it was last written to disk.
    pub needs_save: bool,
    pub fully_updated: bool,
}

impl MapData {
    #[must_use]
    pub fn new(dimension: Dimension, x: i32, z: i32, scale: i8) -> Self {
        Self {
            scale,
            locked: false,
            dimension,
            center_x: x,
            center_z: z,
            tracking_position: true,
            unlimited_tracking: false,
            colors: Box::new([0; MAP_SIZE * MAP_SIZE]),
            decorations: Vec::new(),
            banners: Vec::new(),
            frames: Vec::new(),
            dirty: true,
            needs_save: true,
            fully_updated: false,
        }
    }

    /// Decodes vanilla's `MapItemSavedData.CODEC`.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let dimension = Dimension::from_name(nbt.get_string("dimension")?)?.clone();
        let center_x = nbt.get_numeric_int("xCenter")?;
        let center_z = nbt.get_numeric_int("zCenter")?;
        let stored_colors = nbt.get_byte_array("colors")?;
        let flag = |name: &str, default: bool| {
            nbt.get_numeric_int(name)
                .map_or(default, |value| value != 0)
        };
        let list = |name: &str| {
            nbt.get_list(name)
                .map(<[NbtTag]>::to_vec)
                .unwrap_or_default()
        };
        let mut colors = Box::new([0; MAP_SIZE * MAP_SIZE]);
        // Vanilla keeps a blank map when the stored array has the wrong length.
        if stored_colors.len() == colors.len() {
            for (color, &stored) in colors.iter_mut().zip(stored_colors) {
                *color = stored as u8;
            }
        }
        Some(Self {
            scale: (nbt.get_numeric_int("scale").unwrap_or(0) as i8).clamp(0, MAX_MAP_SCALE),
            locked: flag("locked", false),
            dimension,
            center_x,
            center_z,
            tracking_position: flag("trackingPosition", true),
            unlimited_tracking: flag("unlimitedTracking", false),
            colors,
            decorations: Vec::new(),
            banners: list("banners"),
            frames: list("frames"),
            dirty: true,
            needs_save: false,
            fully_updated: false,
        })
    }

    /// Encodes vanilla's `MapItemSavedData.CODEC`, which leaves out optional fields at their default.
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("dimension", self.dimension.minecraft_name.to_string());
        nbt.put_int("xCenter", self.center_x);
        nbt.put_int("zCenter", self.center_z);
        if self.scale != 0 {
            nbt.put_byte("scale", self.scale);
        }
        nbt.put(
            "colors",
            NbtTag::ByteArray(self.colors.iter().map(|&color| color as i8).collect()),
        );
        if !self.tracking_position {
            nbt.put_bool("trackingPosition", false);
        }
        if self.unlimited_tracking {
            nbt.put_bool("unlimitedTracking", true);
        }
        if self.locked {
            nbt.put_bool("locked", true);
        }
        if !self.banners.is_empty() {
            nbt.put_list("banners", self.banners.clone());
        }
        if !self.frames.is_empty() {
            nbt.put_list("frames", self.frames.clone());
        }
        nbt
    }

    /// `MapItemSavedData.locked`: a locked copy with the same colors and decorations.
    #[must_use]
    pub fn locked(&self) -> Self {
        Self {
            scale: self.scale,
            locked: true,
            dimension: self.dimension.clone(),
            center_x: self.center_x,
            center_z: self.center_z,
            tracking_position: self.tracking_position,
            unlimited_tracking: self.unlimited_tracking,
            colors: self.colors.clone(),
            decorations: self.decorations.clone(),
            // Vanilla copies the banner markers but not the frame markers.
            banners: self.banners.clone(),
            frames: Vec::new(),
            dirty: true,
            needs_save: true,
            fully_updated: false,
        }
    }

    /// `MapItemSavedData.scaled`: a fresh map one scale out, centered on the grid of that scale.
    #[must_use]
    pub fn scaled(&self) -> Self {
        let scale = (self.scale + 1).clamp(0, MAX_MAP_SCALE);
        let (x, z) = fresh_map_center(self.center_x, self.center_z, scale);
        Self {
            tracking_position: self.tracking_position,
            unlimited_tracking: self.unlimited_tracking,
            ..Self::new(self.dimension.clone(), x, z, scale)
        }
    }

    pub fn set_color(&mut self, x: usize, z: usize, color: u8) {
        if x < 128 && z < 128 {
            let idx = z * 128 + x;
            if self.colors[idx] != color {
                self.colors[idx] = color;
                self.dirty = true;
                self.needs_save = true;
            }
        }
    }

    /// Mirrors vanilla `MapItemSavedData.calculateRotation`.
    #[must_use]
    pub fn calculate_rotation(&self, game_time: i64, y_rot: f64) -> i8 {
        if self.dimension == Dimension::THE_NETHER {
            let s = (game_time / NETHER_SPIN_INTERVAL_TICKS) as i32;
            return ((s
                .wrapping_mul(s)
                .wrapping_mul(NETHER_SPIN_HASH_MULTIPLIER)
                .wrapping_add(s.wrapping_mul(NETHER_SPIN_HASH_INCREMENT))
                >> 15)
                & 15) as i8;
        }
        let adjusted_y_rot = if y_rot < 0.0 {
            y_rot - 8.0
        } else {
            y_rot + 8.0
        };
        // Java's (byte) cast narrows through int, wrapping instead of saturating.
        (adjusted_y_rot * 16.0 / 360.0) as i32 as i8
    }

    pub fn update(&mut self, player: &Player) {
        let world = player.world();
        let scale = 1 << self.scale;
        let center_x = self.center_x;
        let center_z = self.center_z;

        let player_pos = player.position();
        let player_x = player_pos.x as i32;
        let player_z = player_pos.z as i32;

        let start_img_x = ((player_x - center_x) / scale + 64).clamp(0, 127) as usize;
        let start_img_z = ((player_z - center_z) / scale + 64).clamp(0, 127) as usize;

        let radius = 16;
        let (range_x, range_z) = if self.fully_updated {
            (
                (start_img_x.saturating_sub(radius))..(start_img_x + radius).min(128),
                (start_img_z.saturating_sub(radius))..(start_img_z + radius).min(128),
            )
        } else {
            self.fully_updated = true;
            (0..128, 0..128)
        };

        for img_x in range_x {
            let mut prev_y = -1;
            for img_z in range_z.clone() {
                let world_x = (img_x as i32 - 64) * scale + center_x;
                let world_z = (img_z as i32 - 64) * scale + center_z;

                let top_y = world.get_top_block(Vector2::new(world_x, world_z));
                let block = world.get_block(&BlockPos::new(world_x, top_y, world_z));

                let color_base = block.map_color;

                let mut brightness = 2; // Normal
                if prev_y != -1 {
                    if top_y > prev_y {
                        brightness = 3; // High
                    } else if top_y < prev_y {
                        brightness = 1; // Low
                    }
                }
                prev_y = top_y;

                let color = color_base * 4 + brightness;
                self.set_color(img_x, img_z, color);
            }
        }
    }
}

#[derive(Clone)]
pub struct MapDecoration {
    pub icon_type: i32,
    pub x: i8,
    pub z: i8,
    pub direction: i8,
    pub display_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 26.3 server writes only the fields that differ from their defaults, and Pumpkin
    /// writes a loaded map back the same way.
    #[test]
    fn vanilla_map_data_round_trips() {
        let mut colors = vec![0i8; MAP_SIZE * MAP_SIZE];
        colors[5] = 34;
        let mut plain = NbtCompound::new();
        plain.put_string("dimension", "minecraft:overworld".to_string());
        plain.put_int("xCenter", 0);
        plain.put_int("zCenter", 0);
        plain.put("colors", NbtTag::ByteArray(colors.into()));
        let map = MapData::from_nbt(&plain).unwrap();
        assert!(map.tracking_position && !map.locked && map.scale == 0);
        assert_eq!(map.colors[5], 34);
        assert_eq!(map.to_nbt(), plain);

        let mut banner = NbtCompound::new();
        banner.put("pos", NbtTag::IntArray(vec![1, 64, 2]));
        banner.put_string("color", "red".to_string());
        let mut full = plain.clone();
        full.put_string("dimension", "minecraft:the_nether".to_string());
        full.put_byte("scale", 3);
        full.put_bool("trackingPosition", false);
        full.put_bool("unlimitedTracking", true);
        full.put_bool("locked", true);
        full.put_list("banners", vec![NbtTag::Compound(banner)]);
        let map = MapData::from_nbt(&full).unwrap();
        assert!(map.locked && map.unlimited_tracking && !map.tracking_position);
        assert_eq!(map.to_nbt(), full);
    }
}
