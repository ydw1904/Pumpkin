use pumpkin_protocol::java::client::play::{
    CInitializeWorldBorder, CSetBorderCenter, CSetBorderLerpSize, CSetBorderSize,
    CSetBorderWarningDelay, CSetBorderWarningDistance,
};

use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_world::world_info::data_files::WorldBorderData;

use crate::net::java::JavaClient;

use super::World;

/// Vanilla `WorldBorder.damagePerBlock` default.
const DEFAULT_DAMAGE_PER_BLOCK: f32 = 0.2;
/// Vanilla `WorldBorder.safeZone` default.
const DEFAULT_SAFE_ZONE: f32 = 5.0;

pub struct Worldborder {
    pub center_x: f64,
    pub center_z: f64,
    pub old_diameter: f64,
    pub new_diameter: f64,
    pub speed: i64,
    pub portal_teleport_boundary: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
    pub damage_per_block: f32,
    pub buffer: f32,
}

impl Worldborder {
    /// The border a dimension's `world_border.dat` describes.
    #[must_use]
    pub const fn from_saved(data: &WorldBorderData) -> Self {
        let mut border = Self::new(
            data.center_x,
            data.center_z,
            data.size,
            data.lerp_time,
            data.warning_blocks,
            data.warning_time,
        );
        if data.lerp_time > 0 {
            border.new_diameter = data.lerp_target;
        }
        border.damage_per_block = data.damage_per_block as f32;
        border.buffer = data.safe_zone as f32;
        border
    }

    /// The border as `world_border.dat` stores it. A size change in progress is saved as
    /// finished, since this server does not track how far it has got.
    #[must_use]
    pub fn to_saved(&self) -> WorldBorderData {
        WorldBorderData {
            center_x: self.center_x,
            center_z: self.center_z,
            damage_per_block: f64::from(self.damage_per_block),
            safe_zone: f64::from(self.buffer),
            warning_blocks: self.warning_blocks,
            warning_time: self.warning_time,
            size: self.new_diameter,
            lerp_time: 0,
            lerp_target: self.new_diameter,
        }
    }

    #[must_use]
    pub const fn new(
        x: f64,
        z: f64,
        diameter: f64,
        speed: i64,
        warning_blocks: i32,
        warning_time: i32,
    ) -> Self {
        Self {
            center_x: x,
            center_z: z,
            old_diameter: diameter,
            new_diameter: diameter,
            speed,
            portal_teleport_boundary: 29_999_984,
            warning_blocks,
            warning_time,
            damage_per_block: DEFAULT_DAMAGE_PER_BLOCK,
            buffer: DEFAULT_SAFE_ZONE,
        }
    }

    pub fn init_client(&self, client: &JavaClient) {
        if let Ok(data) = client.serialize_packet(&CInitializeWorldBorder::new(
            self.center_x,
            self.center_z,
            self.old_diameter,
            self.new_diameter,
            self.speed.into(),
            self.portal_teleport_boundary.into(),
            self.warning_blocks.into(),
            self.warning_time.into(),
        )) {
            client.try_enqueue_packet(data);
        }
    }

    pub fn set_center(&mut self, world: &World, x: f64, z: f64) {
        self.center_x = x;
        self.center_z = z;

        world.broadcast_packet_all(&CSetBorderCenter::new(self.center_x, self.center_z));
    }

    pub fn set_diameter(&mut self, world: &World, diameter: f64, speed: Option<i64>) {
        self.old_diameter = self.new_diameter;
        self.new_diameter = diameter;

        match speed {
            Some(speed) => {
                world.broadcast_packet_all(&CSetBorderLerpSize::new(
                    self.old_diameter,
                    self.new_diameter,
                    speed.into(),
                ));
            }
            None => {
                world.broadcast_packet_all(&CSetBorderSize::new(self.new_diameter));
            }
        }
    }

    pub fn add_diameter(&mut self, world: &World, offset: f64, speed: Option<i64>) {
        self.set_diameter(world, self.new_diameter + offset, speed);
    }

    pub fn set_warning_delay(&mut self, world: &World, delay: i32) {
        self.warning_time = delay;

        world.broadcast_packet_all(&CSetBorderWarningDelay::new(self.warning_time.into()));
    }

    pub fn set_warning_distance(&mut self, world: &World, distance: i32) {
        self.warning_blocks = distance;

        world.broadcast_packet_all(&CSetBorderWarningDistance::new(self.warning_blocks.into()));
    }

    pub const fn set_damage_buffer(&mut self, buffer: f32) {
        self.buffer = buffer;
    }

    pub const fn set_damage_per_block(&mut self, damage: f32) {
        self.damage_per_block = damage;
    }

    pub fn reset(&mut self, world: &World) {
        self.center_x = 0.0;
        self.center_z = 0.0;
        self.old_diameter = 29_999_984.0;
        self.new_diameter = 29_999_984.0;
        self.speed = 0;
        self.portal_teleport_boundary = 29_999_984;
        self.warning_blocks = 5;
        self.warning_time = 15;
        self.damage_per_block = DEFAULT_DAMAGE_PER_BLOCK;
        self.buffer = DEFAULT_SAFE_ZONE;

        world.broadcast_packet_all(&CInitializeWorldBorder::new(
            self.center_x,
            self.center_z,
            self.old_diameter,
            self.new_diameter,
            self.speed.into(),
            self.portal_teleport_boundary.into(),
            self.warning_blocks.into(),
            self.warning_time.into(),
        ));
    }

    #[must_use]
    pub fn contains(&self, x: f64, z: f64) -> bool {
        let half = self.new_diameter / 2.0;
        let min_x = self.center_x - half;
        let max_x = self.center_x + half;
        let min_z = self.center_z - half;
        let max_z = self.center_z + half;
        x >= min_x && x < max_x && z >= min_z && z < max_z
    }

    /// Vanilla `WorldBorder.isWithinBounds(AABB)`.
    #[must_use]
    pub fn contains_box(&self, bounding_box: &BoundingBox) -> bool {
        let epsilon = f64::from(1.0E-5f32);
        self.contains(bounding_box.min.x, bounding_box.min.z)
            && self.contains(bounding_box.max.x - epsilon, bounding_box.max.z - epsilon)
    }

    /// Vanilla `WorldBorder.getDistanceToBorder`: negative outside the border.
    #[must_use]
    pub fn distance_to_border(&self, x: f64, z: f64) -> f64 {
        let half = self.new_diameter / 2.0;
        let from_north = z - (self.center_z - half);
        let from_south = (self.center_z + half) - z;
        let from_west = x - (self.center_x - half);
        let from_east = (self.center_x + half) - x;
        from_west.min(from_east).min(from_north).min(from_south)
    }

    #[must_use]
    pub fn contains_block(&self, x: i32, z: i32) -> bool {
        self.contains(f64::from(x), f64::from(z))
            && self.contains(f64::from(x + 1), f64::from(z + 1))
    }

    #[must_use]
    pub fn clamp_block(&self, x: i32, z: i32) -> (i32, i32) {
        let half = self.new_diameter / 2.0;
        // A border narrower than one block spans no block boundary, leaving `max`
        // below `min`. `Ord::clamp` panics on an inverted range, so collapse the
        // range onto the single block that holds the centre instead.
        let min_x = (self.center_x - half).floor() as i32;
        let max_x = ((self.center_x + half).floor() as i32 - 1).max(min_x);
        let min_z = (self.center_z - half).floor() as i32;
        let max_z = ((self.center_z + half).floor() as i32 - 1).max(min_z);
        (x.clamp(min_x, max_x), z.clamp(min_z, max_z))
    }
}

#[cfg(test)]
mod tests {
    use super::Worldborder;

    fn centered_border(diameter: f64) -> Worldborder {
        Worldborder::new(0.0, 0.0, diameter, 0, 5, 300)
    }

    /// A border set in vanilla has to come back, and go out again, unchanged.
    #[test]
    fn vanilla_world_border_file_round_trips() {
        use pumpkin_nbt::compound::NbtCompound;
        use pumpkin_world::world_info::data_files::read_world_border;

        let dir = tempfile::tempdir().unwrap();
        // The layout a 26.3 server writes after `/worldborder center 10.5 20.5` and `set 5000`.
        let mut data = NbtCompound::new();
        data.put_double("center_x", 10.5);
        data.put_double("center_z", 20.5);
        data.put_double("damage_per_block", 0.5);
        data.put_double("safe_zone", 7.0);
        data.put_int("warning_blocks", 12);
        data.put_int("warning_time", 30);
        data.put_double("size", 5000.0);
        data.put_long("lerp_time", 0);
        data.put_double("lerp_target", 5000.0);
        let mut root = NbtCompound::new();
        root.put_compound("data", data);
        root.put_int("DataVersion", 5023);
        let file = std::fs::File::create(dir.path().join("world_border.dat")).unwrap();
        pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file).unwrap();

        let saved = read_world_border(dir.path()).unwrap();
        let border = Worldborder::from_saved(&saved);

        assert!(border.contains(2510.0, 20.5) && !border.contains(2511.0, 20.5));
        assert_eq!(border.to_saved(), saved);
    }

    /// `create_portal` only falls through to the `clamp_block` fallback
    /// because a zero-width border rejects every candidate it scans: `contains`
    /// reduces to `x >= center && x < center`, which no coordinate satisfies.
    #[test]
    fn a_zero_width_border_contains_no_block() {
        let border = centered_border(0.0);

        assert!(!border.contains_block(0, 0));
        assert!(
            (-32..=32).all(|x| (-32..=32).all(|z| !border.contains_block(x, z))),
            "a zero-width border should contain no block in the portal search area"
        );
    }

    /// `/worldborder set 0` is accepted, because the command's consumer is
    /// `BoundedNumArgumentConsumer::new().min(0.0)` and that bound is inclusive.
    #[test]
    fn clamp_block_handles_a_zero_width_border() {
        let border = centered_border(0.0);

        assert_eq!(border.clamp_block(0, 0), (0, 0));
        assert_eq!(border.clamp_block(100, -100), (0, 0));
    }

    /// A centre off the block grid puts both edges inside the same block, so the
    /// border spans no block boundary even though its diameter is not zero.
    #[test]
    fn clamp_block_handles_a_border_narrower_than_one_block() {
        let border = Worldborder::new(0.5, 0.5, 0.5, 0, 5, 300);

        assert_eq!(border.clamp_block(100, -100), (0, 0));
    }

    #[test]
    fn clamp_block_still_clamps_a_normal_border() {
        let border = centered_border(10.0);

        assert_eq!(border.clamp_block(100, 100), (4, 4));
        assert_eq!(border.clamp_block(-100, -100), (-5, -5));
        assert_eq!(border.clamp_block(2, -3), (2, -3));
    }
}
