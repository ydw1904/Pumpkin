use crate::entity::player::Player;
use crate::server::Server;
use crate::world::World;
use dashmap::DashMap;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{EquipmentType, MapDecorationsImpl, MapIdImpl};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::map_color::MapColor;
use pumpkin_data::map_decoration::MapDecorationType;
use pumpkin_data::tag::{self, Taggable, WorldgenBiome};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_inventory::crafting::recipe_provider::MAX_MAP_SCALE;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CMapItemData, MapIcon, MapPatch};
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::text::TextComponent;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::level::SyncChunk;
use pumpkin_world::world_info::data_files::{read_saved_data, write_saved_data};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::error;
use uuid::Uuid;

/// `MapItemSavedData.MAP_SIZE`.
const MAP_SIZE: usize = 128;
/// `MapItemSavedData.HALF_MAP_SIZE`.
const HALF_MAP_SIZE: i32 = 64;
/// How far from the center, in map pixels, a player still shows as an off-map dot.
const PLAYER_OFF_MAP_RANGE: f32 = 320.0;
/// `HoldingPlayer.nextUpdatePacket` sends changed markers at most every this many updates.
const DECORATION_UPDATE_INTERVAL: i32 = 5;
/// The height `MapItem.update` gives every pixel under a ceiling.
const CEILING_AVERAGE_HEIGHT: f64 = 100.0;
// `MapColor.Brightness` ids.
const LOW: u8 = 0;
const NORMAL: u8 = 1;
const HIGH: u8 = 2;
const LOWEST: u8 = 3;

/// `MapItemSavedData.isInsideMap`: how far from the center, in map pixels, a marker still shows.
const HALF_MAP_INSIDE: f32 = 63.0;

/// Whether the item is a `MapItem`: every map item, and only those, comes with
/// `map_decorations`.
#[must_use]
pub fn is_map_item(item: &Item) -> bool {
    item.components
        .iter()
        .any(|(component, _)| *component == DataComponent::MapDecorations)
}

/// `MapItemSavedData.isInsideMap`.
fn is_inside_map(dx: f32, dz: f32) -> bool {
    (-HALF_MAP_INSIDE..=HALF_MAP_INSIDE).contains(&dx)
        && (-HALF_MAP_INSIDE..=HALF_MAP_INSIDE).contains(&dz)
}

/// `MapItemSavedData.clampMapCoordinate`.
fn clamp_map_coordinate(delta: f32) -> i8 {
    if delta <= -HALF_MAP_INSIDE {
        -128
    } else if delta >= HALF_MAP_INSIDE {
        127
    } else {
        (delta * 2.0 + 0.5) as i32 as i8
    }
}

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
    /// Kept in insertion order like vanilla's `LinkedHashMap`, which is the order they are sent in.
    pub decorations: Vec<MapDecoration>,
    /// Banner and frame markers, kept as saved so vanilla gets them back.
    banners: Vec<NbtTag>,
    frames: Vec<NbtTag>,
    /// `SavedData.isDirty`: the map has changed since it was last written to disk.
    pub needs_save: bool,
    /// `carriedBy`: the players that carry the map, in the order they first did.
    carried_by: Vec<HoldingPlayer>,
}

/// `MapItemSavedData.HoldingPlayer`: what one carrier's client still has to be sent.
struct HoldingPlayer {
    player: Uuid,
    dirty_data: bool,
    min_dirty_x: usize,
    min_dirty_y: usize,
    max_dirty_x: usize,
    max_dirty_y: usize,
    dirty_decorations: bool,
    tick: i32,
    /// Picks which columns `MapData::update` redraws this tick.
    step: i32,
}

impl HoldingPlayer {
    /// A new carrier gets the whole map and every marker first.
    const fn new(player: Uuid) -> Self {
        Self {
            player,
            dirty_data: true,
            min_dirty_x: 0,
            min_dirty_y: 0,
            max_dirty_x: MAP_SIZE - 1,
            max_dirty_y: MAP_SIZE - 1,
            dirty_decorations: true,
            tick: 0,
            step: 0,
        }
    }

    const fn mark_colors_dirty(&mut self, x: usize, y: usize) {
        if self.dirty_data {
            self.min_dirty_x = if x < self.min_dirty_x {
                x
            } else {
                self.min_dirty_x
            };
            self.min_dirty_y = if y < self.min_dirty_y {
                y
            } else {
                self.min_dirty_y
            };
            self.max_dirty_x = if x > self.max_dirty_x {
                x
            } else {
                self.max_dirty_x
            };
            self.max_dirty_y = if y > self.max_dirty_y {
                y
            } else {
                self.max_dirty_y
            };
        } else {
            self.dirty_data = true;
            self.min_dirty_x = x;
            self.min_dirty_y = y;
            self.max_dirty_x = x;
            self.max_dirty_y = y;
        }
    }
}

/// What `tickCarriedBy` needs to know about one carrier, read without the map locked.
struct CarrierState {
    present: bool,
    in_map_dimension: bool,
    x: f64,
    z: f64,
    y_rot: f32,
    game_time: i64,
    name: String,
    hidden: bool,
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
            needs_save: true,
            carried_by: Vec::new(),
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
            needs_save: false,
            carried_by: Vec::new(),
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
            needs_save: true,
            carried_by: Vec::new(),
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

    /// `MapItemSavedData.setColor`.
    pub fn set_color(&mut self, x: usize, z: usize, color: u8) {
        if x < MAP_SIZE && z < MAP_SIZE {
            self.colors[z * MAP_SIZE + x] = color;
            self.needs_save = true;
            for holder in &mut self.carried_by {
                holder.mark_colors_dirty(x, z);
            }
        }
    }

    /// `MapItemSavedData.updateColor`: whether the pixel changed.
    pub fn update_color(&mut self, x: usize, z: usize, color: u8) -> bool {
        if self.colors[z * MAP_SIZE + x] == color {
            return false;
        }
        self.set_color(x, z, color);
        true
    }

    /// Mirrors vanilla `MapItemSavedData.calculateRotation`; `game_time` is `None` where vanilla
    /// passes no level.
    #[must_use]
    pub fn calculate_rotation(&self, game_time: Option<i64>, y_rot: f64) -> i8 {
        if let Some(game_time) = game_time
            && self.dimension == Dimension::THE_NETHER
        {
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

    /// `MapItemSavedData.setDecorationsDirty`.
    fn set_decorations_dirty(&mut self) {
        for holder in &mut self.carried_by {
            holder.dirty_decorations = true;
        }
    }

    /// `MapItemSavedData.removeDecoration`.
    fn remove_decoration(&mut self, key: &str) {
        self.decorations.retain(|decoration| decoration.key != key);
        self.set_decorations_dirty();
    }

    /// `MapItemSavedData.addDecoration`, at world coordinates.
    #[expect(clippy::too_many_arguments, reason = "mirrors vanilla's addDecoration")]
    pub fn add_decoration(
        &mut self,
        decoration_type: &MapDecorationType,
        game_time: Option<i64>,
        key: &str,
        x: f64,
        z: f64,
        y_rot: f64,
        display_name: Option<String>,
    ) {
        let scaling = (1 << self.scale) as f32;
        let dx = (x - f64::from(self.center_x)) as f32 / scaling;
        let dz = (z - f64::from(self.center_z)) as f32 / scaling;
        // `calculateDecorationLocationAndType`.
        let decoration_type = if !is_inside_map(dx, dz)
            && (*decoration_type == MapDecorationType::PLAYER || !self.unlimited_tracking)
        {
            if *decoration_type != MapDecorationType::PLAYER {
                None
            } else if dx.abs() < PLAYER_OFF_MAP_RANGE && dz.abs() < PLAYER_OFF_MAP_RANGE {
                Some(&MapDecorationType::PLAYER_OFF_MAP)
            } else if self.unlimited_tracking {
                Some(&MapDecorationType::PLAYER_OFF_LIMITS)
            } else {
                None
            }
        } else {
            Some(decoration_type)
        };
        let Some(decoration_type) = decoration_type else {
            self.remove_decoration(key);
            return;
        };
        let decoration = MapDecoration {
            key: key.to_string(),
            icon_type: decoration_type.id as i32,
            x: clamp_map_coordinate(dx),
            z: clamp_map_coordinate(dz),
            direction: self.calculate_rotation(game_time, y_rot),
            display_name,
        };
        match self.decorations.iter().position(|d| d.key == key) {
            Some(index) if self.decorations[index] == decoration => return,
            Some(index) => self.decorations[index] = decoration,
            None => self.decorations.push(decoration),
        }
        self.set_decorations_dirty();
    }

    /// The part of `MapItemSavedData.tickCarriedBy` that adds the markers an item carries.
    fn add_static_decorations(&mut self, decorations: &MapDecorationsImpl, game_time: i64) {
        for (key, entry) in &decorations.decorations {
            if self.decorations.iter().any(|d| &d.key == key) {
                continue;
            }
            let name = entry
                .decoration_type
                .strip_prefix("minecraft:")
                .unwrap_or(&entry.decoration_type);
            if let Some(decoration_type) = MapDecorationType::from_name(name) {
                self.add_decoration(
                    decoration_type,
                    Some(game_time),
                    key,
                    entry.x,
                    entry.z,
                    f64::from(entry.rotation),
                    None,
                );
            }
        }
    }

    /// `MapItemSavedData.getHoldingPlayer`.
    fn holding_player(&mut self, player: Uuid) -> &mut HoldingPlayer {
        let index = self
            .carried_by
            .iter()
            .position(|holder| holder.player == player)
            .unwrap_or_else(|| {
                self.carried_by.push(HoldingPlayer::new(player));
                self.carried_by.len() - 1
            });
        &mut self.carried_by[index]
    }

    /// `MapItemSavedData.getUpdatePacket` and `HoldingPlayer.nextUpdatePacket`: sends the player
    /// the pixels and markers that changed since its last update.
    pub fn send_update(&mut self, map_id: i32, player: &Player) {
        let Some(holder) = self
            .carried_by
            .iter_mut()
            .find(|holder| holder.player == player.gameprofile.id)
        else {
            return;
        };
        let patch = holder.dirty_data.then(|| {
            holder.dirty_data = false;
            let width = holder.max_dirty_x + 1 - holder.min_dirty_x;
            let height = holder.max_dirty_y + 1 - holder.min_dirty_y;
            let mut colors = vec![0; width * height];
            for x in 0..width {
                for y in 0..height {
                    colors[x + y * width] =
                        self.colors[holder.min_dirty_x + x + (holder.min_dirty_y + y) * MAP_SIZE];
                }
            }
            (
                holder.min_dirty_x,
                holder.min_dirty_y,
                width,
                height,
                colors,
            )
        });
        let send_decorations = holder.dirty_decorations && {
            let due = holder.tick % DECORATION_UPDATE_INTERVAL == 0;
            holder.tick += 1;
            due
        };
        if send_decorations {
            holder.dirty_decorations = false;
        }
        if patch.is_none() && !send_decorations {
            return;
        }
        let icons: Option<Vec<MapIcon>> = send_decorations.then(|| {
            self.decorations
                .iter()
                .map(|decoration| MapIcon {
                    icon_type: VarInt(decoration.icon_type),
                    x: decoration.x,
                    z: decoration.z,
                    direction: decoration.direction,
                    display_name: decoration
                        .display_name
                        .as_ref()
                        .map(|name| TextComponent::text(name.clone())),
                })
                .collect()
        });
        player.try_send_client_packet(&CMapItemData {
            map_id: VarInt(map_id),
            scale: self.scale,
            tracking_position: self.tracking_position,
            locked: self.locked,
            icons: icons.as_deref(),
            data: patch
                .as_ref()
                .map(|(x, z, width, height, colors)| MapPatch {
                    columns: *width as u8,
                    rows: *height as u8,
                    x: *x as i8,
                    z: *z as i8,
                    data: colors,
                }),
        });
    }

    /// `MapItem.renderBiomePreviewMap`: outlines the water biomes of an exploration map.
    pub fn render_biome_preview(&mut self, world: &crate::world::World) {
        if world.dimension.minecraft_name != self.dimension.minecraft_name {
            return;
        }
        let scale = 1 << self.scale;
        let start_x = self.center_x / scale - MAP_SIZE as i32 / 2;
        let start_z = self.center_z / scale - MAP_SIZE as i32 / 2;
        let positions: Vec<(i32, i32, i32)> = (0..MAP_SIZE as i32)
            .flat_map(|row| {
                (0..MAP_SIZE as i32).map(move |column| {
                    (
                        (start_x + column) * scale,
                        world.sea_level,
                        (start_z + row) * scale,
                    )
                })
            })
            .collect();
        let Some(biomes) = pumpkin_world::generation::generator::biome_finder::sample_biomes(
            &world.level.world_gen(),
            &positions,
        ) else {
            return;
        };
        let watery: Vec<bool> = biomes
            .iter()
            .map(|biome| biome.has_tag(&WorldgenBiome::MINECRAFT_WATER_ON_MAP_OUTLINES))
            .collect();
        // `isBiomeWatery`, indexed by column then row.
        let is_watery = |x: usize, z: usize| watery[z * MAP_SIZE + x];
        for mx in 1..MAP_SIZE - 1 {
            for mz in 1..MAP_SIZE - 1 {
                let mut water_count = 0;
                for (dx, dz) in [
                    (-1, -1),
                    (-1, 0),
                    (-1, 1),
                    (0, -1),
                    (0, 1),
                    (1, -1),
                    (1, 0),
                    (1, 1),
                ] {
                    if is_watery(mx.wrapping_add_signed(dx), mz.wrapping_add_signed(dz)) {
                        water_count += 1;
                    }
                }
                let mut brightness = LOWEST;
                let mut color = None;
                if is_watery(mx, mz) {
                    color = Some(MapColor::COLOR_ORANGE);
                    if water_count > 7 && mz % 2 == 0 {
                        let wave =
                            (mx as i32 + (pumpkin_util::math::sin(mz as f32) * 7.0) as i32) / 8 % 5;
                        brightness = match wave {
                            0 | 4 => LOW,
                            1 | 3 => NORMAL,
                            2 => HIGH,
                            _ => brightness,
                        };
                    } else if water_count > 7 {
                        color = None;
                    } else if water_count > 5 {
                        brightness = NORMAL;
                    } else if water_count > 1 {
                        brightness = LOW;
                    }
                } else if water_count > 0 {
                    color = Some(MapColor::COLOR_BROWN);
                    brightness = if water_count > 3 { NORMAL } else { LOWEST };
                }
                if let Some(color) = color {
                    self.set_color(mx, mz, color.id << 2 | brightness);
                }
            }
        }
    }

    /// `MapItem.update`: redraws one set of columns around a player holding the map.
    pub fn update(&mut self, player: &Player) {
        let world = player.world();
        if world.dimension.minecraft_name != self.dimension.minecraft_name {
            return;
        }
        let scale = 1 << self.scale;
        let center_x = self.center_x;
        let center_z = self.center_z;
        let pos = player.position();
        let player_img_x = (pos.x - f64::from(center_x)).floor() as i32 / scale + HALF_MAP_SIZE;
        let player_img_y = (pos.z - f64::from(center_z)).floor() as i32 / scale + HALF_MAP_SIZE;
        let has_ceiling = world.dimension.has_ceiling;
        let mut radius = MAP_SIZE as i32 / scale;
        if has_ceiling {
            radius /= 2;
        }
        let holder = self.holding_player(player.gameprofile.id);
        holder.step = holder.step.wrapping_add(1);
        let step = holder.step;
        let min_y = world.dimension.min_y;

        let mut found_consecutive_changes = false;
        for img_x in player_img_x - radius + 1..player_img_x + radius {
            if (img_x & 15) != (step & 15) && !found_consecutive_changes {
                continue;
            }
            found_consecutive_changes = false;
            let mut previous_average_area_height = 0.0;
            for img_y in player_img_y - radius - 1..player_img_y + radius {
                if !(0..MAP_SIZE as i32).contains(&img_x) || !(-1..MAP_SIZE as i32).contains(&img_y)
                {
                    continue;
                }
                let distance_to_player_sqr =
                    (img_x - player_img_x).pow(2) + (img_y - player_img_y).pow(2);
                let dither_black = distance_to_player_sqr > (radius - 2) * (radius - 2);
                let area_min_x = (center_x / scale + img_x - HALF_MAP_SIZE) * scale;
                let area_min_z = (center_z / scale + img_y - HALF_MAP_SIZE) * scale;
                // Vanilla loads the chunk here; maps only read chunks that are already loaded.
                let Some((color, average_area_height, water_depth)) = world.level.read_chunk_sync(
                    &Vector2::new(area_min_x >> 4, area_min_z >> 4),
                    |chunk| {
                        if has_ceiling {
                            ceiling_area_color(area_min_x, area_min_z)
                        } else {
                            sample_area(chunk, area_min_x, area_min_z, scale, min_y)
                        }
                    },
                ) else {
                    continue;
                };
                let checker = f64::from((img_x + img_y) & 1);
                let brightness = if color == MapColor::WATER.id {
                    let diff = f64::from(water_depth) * 0.1 + checker * 0.2;
                    if diff < 0.5 {
                        HIGH
                    } else if diff > 0.9 {
                        LOW
                    } else {
                        NORMAL
                    }
                } else {
                    let diff = (average_area_height - previous_average_area_height) * 4.0
                        / f64::from(scale + 4)
                        + (checker - 0.5) * 0.4;
                    if diff > 0.6 {
                        HIGH
                    } else if diff < -0.6 {
                        LOW
                    } else {
                        NORMAL
                    }
                };
                previous_average_area_height = average_area_height;
                if img_y >= 0
                    && distance_to_player_sqr < radius * radius
                    && (!dither_black || (img_x + img_y) & 1 != 0)
                {
                    found_consecutive_changes |=
                        self.update_color(img_x as usize, img_y as usize, color << 2 | brightness);
                }
            }
        }
    }
}

/// The Nether part of `MapItem.update`: a fixed dirt and stone pattern under the ceiling.
fn ceiling_area_color(area_min_x: i32, area_min_z: i32) -> (u8, f64, i32) {
    let mut noise = area_min_x.wrapping_add(area_min_z.wrapping_mul(231_871));
    noise = noise
        .wrapping_mul(noise)
        .wrapping_mul(31_287_121)
        .wrapping_add(noise.wrapping_mul(11));
    let color = if (noise >> 20) & 1 == 0 {
        Block::DIRT.map_color
    } else {
        Block::STONE.map_color
    };
    (color, CEILING_AVERAGE_HEIGHT, 0)
}

/// The column scan of `MapItem.update` for one map pixel: the most common map color of its
/// `scale * scale` columns, their average height and the average water depth.
fn sample_area(
    chunk: &SyncChunk,
    area_min_x: i32,
    area_min_z: i32,
    scale: i32,
    min_y: i32,
) -> (u8, f64, i32) {
    let block_at = |x: i32, y: i32, z: i32| {
        chunk
            .section
            .get_block_absolute_y((x & 15) as usize, y, (z & 15) as usize)
            .unwrap_or(Block::AIR.default_state.id)
    };
    // `Multisets.copyHighestCountFirst` on a `LinkedHashMultiset`: ties go to the first color seen.
    let mut color_count: Vec<(u8, i32)> = Vec::new();
    let mut water_depth = 0;
    let mut average_area_height = 0.0;
    for dx in 0..scale {
        for dz in 0..scale {
            let x = area_min_x + dx;
            let z = area_min_z + dz;
            let surface = chunk
                .heightmap
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(ChunkHeightmapType::WorldSurface, x, z, min_y);
            let mut column_y = surface + 1;
            let state = if column_y <= min_y {
                Block::BEDROCK.default_state.id
            } else {
                let mut state;
                loop {
                    column_y -= 1;
                    state = block_at(x, column_y, z);
                    if state.to_block().map_color != MapColor::NONE_0.id || column_y <= min_y {
                        break;
                    }
                }
                let fluid = World::get_fluid_from_state_id(state);
                if column_y > min_y && fluid != &Fluid::EMPTY {
                    let mut solid_y = column_y - 1;
                    loop {
                        let below = block_at(x, solid_y, z);
                        solid_y -= 1;
                        water_depth += 1;
                        if solid_y <= min_y
                            || World::get_fluid_from_state_id(below) == &Fluid::EMPTY
                        {
                            break;
                        }
                    }
                    // `getCorrectStateForFluidBlock`.
                    if !state.is_side_solid(BlockDirection::Up) {
                        state = if fluid.matches_type(&Fluid::LAVA) {
                            Block::LAVA.default_state.id
                        } else {
                            Block::WATER.default_state.id
                        };
                    }
                }
                state
            };
            average_area_height += f64::from(column_y) / f64::from(scale * scale);
            let color = state.to_block().map_color;
            match color_count.iter_mut().find(|(seen, _)| *seen == color) {
                Some((_, count)) => *count += 1,
                None => color_count.push((color, 1)),
            }
        }
    }
    water_depth /= scale * scale;
    let mut color = MapColor::NONE_0.id;
    let mut best = 0;
    for (seen, count) in color_count {
        if count > best {
            color = seen;
            best = count;
        }
    }
    (color, average_area_height, water_depth)
}

/// `MapItem.inventoryTick`: a map anywhere in a player's inventory keeps its markers up to date,
/// and one in a hand also draws the terrain around the player.
pub fn inventory_tick(server: &Server, player: &Player, stack: &ItemStack, in_hand: bool) {
    let Some(map_id) = stack.get_data_component::<MapIdImpl>().map(|id| id.id) else {
        return;
    };
    let Some(map) = server.map_manager.get_map(map_id) else {
        return;
    };
    tick_carried_by(server, &map, player, stack, map_id);
    let mut map = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !map.locked && in_hand {
        map.update(player);
    }
}

/// `MapItemSavedData.tickCarriedBy` for a map in a player's inventory (frames aren't ported).
fn tick_carried_by(
    server: &Server,
    map: &Mutex<MapData>,
    ticking_player: &Player,
    stack: &ItemStack,
    map_id: i32,
) {
    let (holders, dimension): (Vec<Uuid>, _) = {
        let mut map = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.holding_player(ticking_player.gameprofile.id);
        (
            map.carried_by.iter().map(|holder| holder.player).collect(),
            map.dimension.minecraft_name,
        )
    };
    // Other carriers' inventories are read before the map is locked again.
    let carriers: Vec<(Uuid, CarrierState)> = holders
        .into_iter()
        .map(|uuid| {
            let state = server.get_player_by_uuid(uuid).map_or_else(
                || CarrierState {
                    present: false,
                    in_map_dimension: false,
                    x: 0.0,
                    z: 0.0,
                    y_rot: 0.0,
                    game_time: 0,
                    name: String::new(),
                    hidden: false,
                },
                |player| {
                    let world = player.world();
                    let pos = player.position();
                    CarrierState {
                        present: !player.living_entity.entity.is_removed()
                            && carries_map(&player, stack.item, map_id),
                        in_map_dimension: world.dimension.minecraft_name == dimension,
                        x: pos.x,
                        z: pos.z,
                        y_rot: player.living_entity.entity.yaw.load(),
                        game_time: world.get_world_age(),
                        name: player.gameprofile.name.clone(),
                        hidden: has_map_invisibility_item_equipped(&player),
                    }
                },
            );
            (uuid, state)
        })
        .collect();
    let ticking_carries = carriers
        .iter()
        .find(|(uuid, _)| *uuid == ticking_player.gameprofile.id)
        .is_some_and(|(_, state)| state.present);

    let mut map = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ticking_carries {
        map.remove_decoration(&ticking_player.gameprofile.name);
    }
    // Vanilla walks `carriedBy` by index and removes from it as it goes, so the carrier after a
    // removed one waits until the next tick.
    let mut index = 0;
    while index < map.carried_by.len() {
        let uuid = map.carried_by[index].player;
        let Some((_, carrier)) = carriers.iter().find(|(seen, _)| *seen == uuid) else {
            index += 1;
            continue;
        };
        if carrier.present {
            if carrier.in_map_dimension && map.tracking_position {
                map.add_decoration(
                    &MapDecorationType::PLAYER,
                    Some(carrier.game_time),
                    &carrier.name,
                    carrier.x,
                    carrier.z,
                    f64::from(carrier.y_rot),
                    None,
                );
            }
        } else {
            map.carried_by.remove(index);
            map.remove_decoration(&carrier.name);
        }
        if uuid != ticking_player.gameprofile.id && carrier.hidden {
            map.remove_decoration(&carrier.name);
        }
        index += 1;
    }
    if let Some(decorations) = stack.get_data_component::<MapDecorationsImpl>() {
        map.add_static_decorations(decorations, ticking_player.world().get_world_age());
    }
}

/// `Inventory.contains(MapItemSavedData.mapMatcher)`: the same map item with the same id.
fn carries_map(player: &Player, item: &Item, map_id: i32) -> bool {
    let matches = |stack: &ItemStack| {
        stack.item.id == item.id
            && stack
                .get_data_component::<MapIdImpl>()
                .is_some_and(|id| id.id == map_id)
    };
    let inventory = player.inventory();
    if inventory
        .main_inventory
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(matches)
    {
        return true;
    }
    inventory
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .equipment
        .values()
        .any(matches)
}

/// `MapItemSavedData.hasMapInvisibilityItemEquipped`: a carved pumpkin on the head hides the
/// wearer from other players' maps.
fn has_map_invisibility_item_equipped(player: &Player) -> bool {
    player
        .inventory()
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .equipment
        .iter()
        .any(|(slot, stack)| {
            slot.slot_type() != EquipmentType::Hand
                && stack
                    .item
                    .has_tag(&tag::Item::MINECRAFT_MAP_INVISIBILITY_EQUIPMENT)
        })
}

#[derive(Clone, PartialEq, Eq)]
pub struct MapDecoration {
    /// The key vanilla keeps the decoration under, like `+` for an exploration map's target.
    pub key: String,
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
