use core::f32;
use pumpkin_util::random::gameplay::game_random;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use pumpkin_data::BlockDirection;
use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::painting_variant::PaintingVariant;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::version::JavaMinecraftVersion;

/// How far a painting sits from the center of its block towards the wall (`shiftToBlockWall`).
const SHIFT_TO_BLOCK_WALL: f64 = 0.46875;

/// The world stores a painting facing as a horizontal value: 0 south, 1 west,
/// 2 north, 3 east (`Direction.get2DDataValue`). The entity data, and the spawn
/// packet built from it, use the 3D index instead: north 2, south 3, west 4,
/// east 5. Sending the raw horizontal value makes the client read 0 as down,
/// fail the horizontal check in `HangingEntity.setDirection` and disconnect.
const fn facing_from_horizontal(value: u8) -> BlockDirection {
    match value & 3 {
        1 => BlockDirection::West,
        2 => BlockDirection::North,
        3 => BlockDirection::East,
        _ => BlockDirection::South,
    }
}

/// Vanilla reads the field with `getByte`, which yields 0 (south) when it is
/// absent, so a partial entity file keeps a horizontal default.
fn facing_from_nbt(nbt: &NbtCompound) -> BlockDirection {
    facing_from_horizontal(nbt.get_byte("facing").unwrap_or(0) as u8)
}

const fn facing_to_horizontal(direction: BlockDirection) -> u8 {
    match direction {
        BlockDirection::West => 1,
        BlockDirection::North => 2,
        BlockDirection::East => 3,
        _ => 0,
    }
}

pub struct PaintingEntity {
    pub entity: Entity,
    variant_id: AtomicU32,
}

impl PaintingEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self::new_with_variant(entity, PaintingVariant::Alban)
    }

    #[must_use]
    pub const fn new_with_variant(entity: Entity, variant: PaintingVariant) -> Self {
        Self {
            entity,
            variant_id: AtomicU32::new(variant.id()),
        }
    }

    #[must_use]
    pub fn variant(&self) -> PaintingVariant {
        let id = self.variant_id.load(Ordering::Relaxed);
        PaintingVariant::all()
            .get(id as usize)
            .copied()
            .unwrap_or(PaintingVariant::Alban)
    }

    pub fn set_variant(&self, variant: PaintingVariant) {
        self.variant_id.store(variant.id(), Ordering::Relaxed);
        self.sync_variant();
    }

    pub fn sync_variant(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::painting::DATA_PAINTING_VARIANT_ID,
            VarInt(self.variant_id.load(Ordering::Relaxed) as i32),
        );
    }

    /// Calculates the exact floating-point center position for a painting of given width and height (in blocks),
    /// placed on `face` of `location`.
    #[must_use]
    pub fn calculate_center_pos(
        location: BlockPos,
        face: BlockDirection,
        width: u32,
        height: u32,
    ) -> Vector3<f64> {
        let target_pos = location.offset(face.to_offset());
        let face_ccw = face.rotate_counter_clockwise();
        let step_x = f64::from(face.to_offset().x);
        let step_z = f64::from(face.to_offset().z);
        let ccw_step_x = f64::from(face_ccw.to_offset().x);
        let ccw_step_z = f64::from(face_ccw.to_offset().z);

        let mut x = f64::from(target_pos.0.x) + 0.5 - step_x * SHIFT_TO_BLOCK_WALL;
        let mut y = f64::from(target_pos.0.y) + 0.5;
        let mut z = f64::from(target_pos.0.z) + 0.5 - step_z * SHIFT_TO_BLOCK_WALL;

        let width_offset = if width.is_multiple_of(2) { 0.5 } else { 0.0 };
        let height_offset = if height.is_multiple_of(2) { 0.5 } else { 0.0 };

        x += width_offset * ccw_step_x;
        z += width_offset * ccw_step_z;
        y += height_offset;

        Vector3::new(x, y, z)
    }

    /// The block the painting hangs in (vanilla's `BlockAttachedEntity.pos`), worked back
    /// from its center the way [`Self::calculate_center_pos`] places it.
    fn attached_block_pos(
        center: Vector3<f64>,
        face: BlockDirection,
        width: u32,
        height: u32,
    ) -> BlockPos {
        let face_step = face.to_offset();
        let ccw_step = face.rotate_counter_clockwise().to_offset();
        let width_offset = if width.is_multiple_of(2) { 0.5 } else { 0.0 };
        let height_offset = if height.is_multiple_of(2) { 0.5 } else { 0.0 };
        BlockPos::new(
            (center.x + f64::from(face_step.x) * SHIFT_TO_BLOCK_WALL
                - width_offset * f64::from(ccw_step.x))
            .floor() as i32,
            (center.y - height_offset).floor() as i32,
            (center.z + f64::from(face_step.z) * SHIFT_TO_BLOCK_WALL
                - width_offset * f64::from(ccw_step.z))
            .floor() as i32,
        )
    }

    /// Checks if a painting of the specified variant fits on the wall at `location` facing `face`.
    /// The wall blocks behind the painting must be solid, and the blocks in front must not be solid.
    #[must_use]
    pub fn painting_fits(
        world: &crate::world::World,
        location: BlockPos,
        face: BlockDirection,
        variant: PaintingVariant,
    ) -> bool {
        let width = variant.width();
        let height = variant.height();
        let face_ccw = face.rotate_counter_clockwise();
        let ccw_offset = face_ccw.to_offset();
        let face_offset = face.to_offset();

        let k = -((width as i32 - 1) / 2);
        let l = -((height as i32 - 1) / 2);

        for x_idx in 0..width {
            for y_idx in 0..height {
                let wall_pos = BlockPos(Vector3::new(
                    location.0.x + ccw_offset.x * (x_idx as i32 + k),
                    location.0.y + (y_idx as i32 + l),
                    location.0.z + ccw_offset.z * (x_idx as i32 + k),
                ));
                let front_pos = wall_pos.offset(face_offset);

                let wall_state = world.get_block_state(&wall_pos);
                if !wall_state.is_solid() {
                    return false;
                }

                let front_state = world.get_block_state(&front_pos);
                if front_state.is_solid() {
                    return false;
                }
            }
        }
        true
    }

    /// Chooses a placeable painting variant that fits the wall at `location` facing `face`.
    /// Matches vanilla Minecraft: filters to variants that fit, selects the subset with
    /// the largest area (width * height), and randomly picks one from that subset.
    #[must_use]
    pub fn choose_variant(
        world: &crate::world::World,
        location: BlockPos,
        face: BlockDirection,
    ) -> Option<PaintingVariant> {
        let mut fitting_variants = Vec::new();
        let mut max_area = 0;

        for &variant in PaintingVariant::all_placeable() {
            if Self::painting_fits(world, location, face, variant) {
                let area = variant.width() * variant.height();
                if area > max_area {
                    max_area = area;
                    fitting_variants.clear();
                    fitting_variants.push(variant);
                } else if area == max_area {
                    fitting_variants.push(variant);
                }
            }
        }

        game_random().choose(&fitting_variants).copied()
    }

    fn drop_and_remove(&self, caused_by: Option<&dyn EntityBase>) {
        let entity = &self.entity;
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityPaintingBreak,
            SoundCategory::Blocks,
            &entity.pos.load(),
        );

        let is_creative = caused_by.is_some_and(|c| {
            c.cast_any()
                .downcast_ref::<Player>()
                .is_some_and(Player::is_creative)
        });

        if !is_creative && world.level_info.load().game_rules.entity_drops {
            entity.spawn_at_location(ItemStack::new(1, &Item::PAINTING));
        }

        entity.remove();
    }
}

impl EntityBase for PaintingEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let index = self.entity.data.load(Ordering::Relaxed) as u8;
        let direction = BlockDirection::from_index(index).unwrap_or(BlockDirection::South);
        nbt.put_byte("facing", facing_to_horizontal(direction) as i8);
        let variant_id = self.variant_id.load(Ordering::Relaxed);
        let variant_str =
            self.entity
                .resolve_dynamic_entry_id("minecraft:painting_variant", variant_id, |_id| {
                    self.variant().asset_id().to_string()
                });
        nbt.put_string("variant", variant_str);
        let variant = self.variant();
        super::write_block_pos(
            nbt,
            Self::attached_block_pos(
                self.entity.pos.load(),
                direction,
                variant.width(),
                variant.height(),
            ),
        );
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let facing = facing_from_nbt(nbt);
        self.entity
            .data
            .store(i32::from(facing.to_index()), Ordering::Relaxed);
        if let Some(variant_str) = nbt
            .get_string("variant")
            .or_else(|| nbt.get_string("Motive"))
            && let Some(raw_id) =
                self.entity
                    .resolve_dynamic_raw_id("minecraft:painting_variant", variant_str, |n| {
                        PaintingVariant::from_name(n).map(|v| v.id())
                    })
        {
            self.variant_id.store(raw_id, Ordering::Relaxed);
            self.sync_variant();
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn init_data_tracker(&self) {
        self.sync_variant();
    }

    fn java_spawn_metadata(&self, version: JavaMinecraftVersion) -> Option<Box<[u8]>> {
        let mut metadata = Vec::new();
        Metadata::new(
            pumpkin_data::tracked_data::painting::DATA_PAINTING_VARIANT_ID,
            VarInt(self.variant_id.load(Ordering::Relaxed) as i32),
        )
        .write(&mut metadata, &version)
        .ok()?;
        metadata.push(255);
        Some(metadata.into_boxed_slice())
    }

    fn set_variant_name(&self, name: &str) {
        if let Some(variant) = PaintingVariant::from_name(name) {
            self.set_variant(variant);
        } else if let Some(server) = self.entity.world.load().server.upgrade()
            && let Some(raw_id) = server
                .datapack_manager
                .get_dynamic_registry_raw_id("minecraft:painting_variant", name)
        {
            self.variant_id.store(raw_id, Ordering::Relaxed);
            self.sync_variant();
        }
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        // Vanilla `BlockAttachedEntity.hurtServer`: drops are decided by the attacker, not by
        // the projectile that hit.
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        let mob_griefing = self
            .entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .mob_griefing;
        if !mob_griefing && cause.is_some_and(|cause| cause.get_mob().is_some()) {
            return false;
        }
        if !self.entity.is_removed() {
            self.drop_and_remove(cause);
        }
        true
    }

    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vanilla moves a loaded painting to wherever `block_pos` puts it, so the saved
    /// block has to give back the painting's own center for every size parity.
    #[test]
    fn saved_block_pos_places_the_painting_where_it_was() {
        let wall = BlockPos::new(10, 64, -7);
        for face in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::West,
            BlockDirection::East,
        ] {
            for (width, height) in [(1, 1), (2, 1), (1, 2), (4, 3), (4, 4)] {
                let center = PaintingEntity::calculate_center_pos(wall, face, width, height);
                assert_eq!(
                    PaintingEntity::attached_block_pos(center, face, width, height),
                    wall.offset(face.to_offset()),
                    "{face:?} {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn horizontal_and_3d_facings_round_trip() {
        for horizontal in 0u8..4 {
            let direction = facing_from_horizontal(horizontal);
            assert!(direction.is_horizontal(), "{direction:?} is vertical");
            assert_eq!(facing_to_horizontal(direction), horizontal);
        }
    }

    #[test]
    fn south_two_is_not_down_zero() {
        // The old bug: south (0 in the world file) was sent as index 0, which the
        // client reads as down and rejects.
        assert_eq!(facing_from_horizontal(0), BlockDirection::South);
        assert_eq!(facing_from_horizontal(0).to_index(), 3);
    }

    #[test]
    fn missing_facing_defaults_to_south() {
        let nbt = NbtCompound::new();
        assert_eq!(facing_from_nbt(&nbt), BlockDirection::South);
        assert_eq!(facing_from_nbt(&nbt).to_index(), 3);

        let mut with_facing = NbtCompound::new();
        with_facing.put_byte("facing", 1);
        assert_eq!(facing_from_nbt(&with_facing), BlockDirection::West);
    }

    #[test]
    fn out_of_range_values_wrap_into_horizontal_directions() {
        assert!(facing_from_horizontal(4).is_horizontal());
        assert!(facing_from_horizontal(255).is_horizontal());
    }

    #[test]
    fn calculate_center_pos_works() {
        let location = BlockPos(Vector3::new(10, 64, 20));

        // 1x1 painting facing North: placed against block at (10, 64, 20)
        let pos_1x1 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 1, 1);
        assert_eq!(pos_1x1.x, 10.5);
        assert_eq!(pos_1x1.y, 64.5);
        assert!((pos_1x1.z - 19.96875).abs() < 1e-6);

        // 2x1 painting facing North: width is 2, shifts along counter-clockwise (West = -X)
        let pos_2x1 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 2, 1);
        assert_eq!(pos_2x1.x, 10.0);
        assert_eq!(pos_2x1.y, 64.5);
        assert!((pos_2x1.z - 19.96875).abs() < 1e-6);

        // 2x2 painting facing North: height is 2, shifts +0.5 along Y
        let pos_2x2 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 2, 2);
        assert_eq!(pos_2x2.x, 10.0);
        assert_eq!(pos_2x2.y, 65.0);
        assert!((pos_2x2.z - 19.96875).abs() < 1e-6);

        // 4x4 painting facing North
        let pos_4x4 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 4, 4);
        assert_eq!(pos_4x4.x, 10.0);
        assert_eq!(pos_4x4.y, 65.0);
        assert!((pos_4x4.z - 19.96875).abs() < 1e-6);

        // 1x1 painting facing South: placed on south face of (10, 64, 20) -> target is (10, 64, 21)
        let pos_south = PaintingEntity::calculate_center_pos(location, BlockDirection::South, 1, 1);
        assert_eq!(pos_south.x, 10.5);
        assert_eq!(pos_south.y, 64.5);
        assert!((pos_south.z - 21.03125).abs() < 1e-6);
    }

    #[test]
    fn variant_resolution() {
        assert_eq!(
            PaintingVariant::from_name("minecraft:kebab"),
            Some(PaintingVariant::Kebab)
        );
        assert_eq!(
            PaintingVariant::from_name("kebab"),
            Some(PaintingVariant::Kebab)
        );
        assert_eq!(PaintingVariant::from_name("invalid"), None);
        assert_eq!(PaintingVariant::Kebab.asset_id(), "minecraft:kebab");
    }
}
