use crate::entity::player::Player;
use dashmap::DashMap;
use pumpkin_data::dimension::Dimension;
use pumpkin_inventory::crafting::recipe_provider::MAX_MAP_SCALE;
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use std::sync::{Arc, Mutex};

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

pub struct MapManager {
    pub maps: DashMap<i32, Arc<Mutex<MapData>>>,
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MapManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            maps: DashMap::new(),
        }
    }

    #[must_use]
    pub fn get_map(&self, id: i32) -> Option<Arc<Mutex<MapData>>> {
        self.maps.get(&id).map(|m| m.clone())
    }

    pub fn insert_map(&self, id: i32, data: MapData) {
        self.maps.insert(id, Arc::new(Mutex::new(data)));
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
        self.maps.insert(id, map.clone());
        map
    }
}

pub struct MapData {
    pub scale: i8,
    pub locked: bool,
    pub dimension: Dimension,
    pub center_x: i32,
    pub center_z: i32,
    pub colors: Box<[u8; 128 * 128]>,
    pub decorations: Vec<MapDecoration>,
    pub dirty: bool,
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
            colors: Box::new([0; 128 * 128]),
            decorations: Vec::new(),
            dirty: true,
            fully_updated: false,
        }
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
            colors: self.colors.clone(),
            decorations: self.decorations.clone(),
            dirty: true,
            fully_updated: false,
        }
    }

    /// `MapItemSavedData.scaled`: a fresh map one scale out, centered on the grid of that scale.
    #[must_use]
    pub fn scaled(&self) -> Self {
        let scale = (self.scale + 1).clamp(0, MAX_MAP_SCALE);
        let (x, z) = fresh_map_center(self.center_x, self.center_z, scale);
        Self::new(self.dimension.clone(), x, z, scale)
    }

    pub fn set_color(&mut self, x: usize, z: usize, color: u8) {
        if x < 128 && z < 128 {
            let idx = z * 128 + x;
            if self.colors[idx] != color {
                self.colors[idx] = color;
                self.dirty = true;
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
