use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI8, AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::{BlockStateId, damage::DamageType, item_stack::ItemStack};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{
    codec::{item_stack_seralizer::ItemStackSerializer, var_int::VarInt},
    java::client::play::MetadataSerializer,
    ser::{NetworkWriteExt, WritingError},
};
use pumpkin_util::{
    math::{vector2::Vector2, vector3::Vector3},
    text::TextComponent,
};
use pumpkin_world::generation::structure::template::PaletteEntry;
use tracing::warn;

use crate::{
    command::{CommandSender, CommandSource, text_resolution},
    entity::{
        Entity, EntityBase, decoration::transformation::Transformation, living::LivingEntity,
    },
    server::Server,
};

/// Vanilla clamps `teleport_duration` to `0..=59`.
const MAX_TELEPORT_DURATION: i32 = 59;
const MAX_LIGHT_LEVEL: i32 = 15;

/// `LightCoordsUtil.pack`, the form display brightness is synced in.
const fn pack_brightness(block: i32, sky: i32) -> i32 {
    block << 4 | sky << 20
}

#[derive(Clone, Copy, Debug)]
pub struct Vector3fSerializer(pub f32, pub f32, pub f32);

impl MetadataSerializer for Vector3fSerializer {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        writer.write_f32(self.0)?;
        writer.write_f32(self.1)?;
        writer.write_f32(self.2)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QuaternionfSerializer(pub f32, pub f32, pub f32, pub f32);

impl MetadataSerializer for QuaternionfSerializer {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        writer.write_f32(self.0)?;
        writer.write_f32(self.1)?;
        writer.write_f32(self.2)?;
        writer.write_f32(self.3)
    }
}

pub struct DisplayEntity {
    pub entity: Entity,
    pub interpolation_start_delta_ticks: AtomicI32,
    pub interpolation_duration: AtomicI32,
    pub teleport_duration: AtomicI32,
    pub view_range: Mutex<f32>,
    pub shadow_radius: Mutex<f32>,
    pub shadow_strength: Mutex<f32>,
    pub width: Mutex<f32>,
    pub height: Mutex<f32>,
    pub glow_color_override: AtomicI32,
    pub billboard: AtomicU8,
    pub brightness: AtomicI32,
    pub translation: Mutex<Vector3<f32>>,
    pub scale: Mutex<Vector3<f32>>,
    pub left_rotation: Mutex<[f32; 4]>,
    pub right_rotation: Mutex<[f32; 4]>,
}

impl DisplayEntity {
    pub fn new(entity: Entity) -> Self {
        entity.no_physics.store(true, Ordering::Relaxed);
        // Vanilla `Display.defineSynchedData`.
        let data = &entity.synched_data;
        data.define(
            pumpkin_data::tracked_data::display::DATA_POS_ROT_INTERPOLATION_DURATION_ID,
            VarInt(0),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_TRANSFORMATION_INTERPOLATION_START_DELTA_TICKS_ID,
            VarInt(0),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_TRANSFORMATION_INTERPOLATION_DURATION_ID,
            VarInt(0),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_TRANSLATION_ID,
            Vector3fSerializer(0.0, 0.0, 0.0),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_SCALE_ID,
            Vector3fSerializer(1.0, 1.0, 1.0),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_RIGHT_ROTATION_ID,
            QuaternionfSerializer(0.0, 0.0, 0.0, 1.0),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_LEFT_ROTATION_ID,
            QuaternionfSerializer(0.0, 0.0, 0.0, 1.0),
        );
        // `BillboardConstraints.FIXED`.
        data.define(
            pumpkin_data::tracked_data::display::DATA_BILLBOARD_RENDER_CONSTRAINTS_ID,
            0u8,
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_BRIGHTNESS_OVERRIDE_ID,
            VarInt(-1),
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_VIEW_RANGE_ID,
            1.0f32,
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_SHADOW_RADIUS_ID,
            0.0f32,
        );
        data.define(
            pumpkin_data::tracked_data::display::DATA_SHADOW_STRENGTH_ID,
            1.0f32,
        );
        data.define(pumpkin_data::tracked_data::display::DATA_WIDTH_ID, 0.0f32);
        data.define(pumpkin_data::tracked_data::display::DATA_HEIGHT_ID, 0.0f32);
        data.define(
            pumpkin_data::tracked_data::display::DATA_GLOW_COLOR_OVERRIDE_ID,
            VarInt(-1),
        );
        Self {
            entity,
            interpolation_start_delta_ticks: AtomicI32::new(0),
            interpolation_duration: AtomicI32::new(0),
            teleport_duration: AtomicI32::new(0),
            view_range: Mutex::new(1.0),
            shadow_radius: Mutex::new(0.0),
            shadow_strength: Mutex::new(1.0),
            width: Mutex::new(0.0),
            height: Mutex::new(0.0),
            glow_color_override: AtomicI32::new(-1),
            billboard: AtomicU8::new(0),
            brightness: AtomicI32::new(-1),
            translation: Mutex::new(Vector3::new(0.0, 0.0, 0.0)),
            scale: Mutex::new(Vector3::new(1.0, 1.0, 1.0)),
            left_rotation: Mutex::new([0.0, 0.0, 0.0, 1.0]),
            right_rotation: Mutex::new([0.0, 0.0, 0.0, 1.0]),
        }
    }

    pub fn get_interpolation_start_delta_ticks(&self) -> i32 {
        self.interpolation_start_delta_ticks.load(Ordering::Relaxed)
    }

    pub fn set_interpolation_start_delta_ticks(&self, ticks: i32) {
        self.interpolation_start_delta_ticks
            .store(ticks, Ordering::Relaxed);
        // Vanilla `setTransformationInterpolationDelay` forces the update even when unchanged.
        self.entity.synched_data.set_forced(
            pumpkin_data::tracked_data::display::START_INTERPOLATION,
            VarInt(ticks),
        );
    }

    pub fn get_interpolation_duration(&self) -> i32 {
        self.interpolation_duration.load(Ordering::Relaxed)
    }

    pub fn set_interpolation_duration(&self, duration: i32) {
        self.interpolation_duration
            .store(duration, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::INTERPOLATION_DURATION,
            VarInt(duration),
        );
    }

    pub fn get_teleport_duration(&self) -> i32 {
        self.teleport_duration.load(Ordering::Relaxed)
    }

    pub fn set_teleport_duration(&self, duration: i32) {
        self.teleport_duration.store(duration, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::TELEPORT_DURATION,
            VarInt(duration),
        );
    }

    pub fn get_translation(&self) -> Vector3<f32> {
        *self
            .translation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_translation(&self, translation: Vector3<f32>) {
        *self
            .translation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = translation;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::TRANSLATION,
            Vector3fSerializer(translation.x, translation.y, translation.z),
        );
    }

    pub fn get_scale(&self) -> Vector3<f32> {
        *self
            .scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_scale(&self, scale: Vector3<f32>) {
        *self
            .scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = scale;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::SCALE,
            Vector3fSerializer(scale.x, scale.y, scale.z),
        );
    }

    pub fn get_left_rotation(&self) -> [f32; 4] {
        *self
            .left_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_left_rotation(&self, left_rotation: [f32; 4]) {
        *self
            .left_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = left_rotation;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::LEFT_ROTATION,
            QuaternionfSerializer(
                left_rotation[0],
                left_rotation[1],
                left_rotation[2],
                left_rotation[3],
            ),
        );
    }

    pub fn get_right_rotation(&self) -> [f32; 4] {
        *self
            .right_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_right_rotation(&self, right_rotation: [f32; 4]) {
        *self
            .right_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = right_rotation;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::RIGHT_ROTATION,
            QuaternionfSerializer(
                right_rotation[0],
                right_rotation[1],
                right_rotation[2],
                right_rotation[3],
            ),
        );
    }

    pub fn get_billboard(&self) -> u8 {
        self.billboard.load(Ordering::Relaxed)
    }

    pub fn set_billboard(&self, billboard: u8) {
        self.billboard.store(billboard, Ordering::Relaxed);
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::BILLBOARD, billboard);
    }

    pub fn get_brightness(&self) -> i32 {
        self.brightness.load(Ordering::Relaxed)
    }

    pub fn set_brightness(&self, brightness: i32) {
        self.brightness.store(brightness, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::BRIGHTNESS,
            VarInt(brightness),
        );
    }

    pub fn get_view_range(&self) -> f32 {
        *self
            .view_range
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_view_range(&self, view_range: f32) {
        *self
            .view_range
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = view_range;
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::VIEW_RANGE, view_range);
    }

    pub fn get_shadow_radius(&self) -> f32 {
        *self
            .shadow_radius
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_shadow_radius(&self, shadow_radius: f32) {
        *self
            .shadow_radius
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = shadow_radius;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::SHADOW_RADIUS,
            shadow_radius,
        );
    }

    pub fn get_shadow_strength(&self) -> f32 {
        *self
            .shadow_strength
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_shadow_strength(&self, shadow_strength: f32) {
        *self
            .shadow_strength
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = shadow_strength;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::SHADOW_STRENGTH,
            shadow_strength,
        );
    }

    pub fn get_display_width(&self) -> f32 {
        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_display_width(&self, width: f32) {
        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = width;
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::WIDTH, width);
    }

    pub fn get_display_height(&self) -> f32 {
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_display_height(&self, height: f32) {
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = height;
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::HEIGHT, height);
    }

    pub fn get_glow_color_override(&self) -> i32 {
        self.glow_color_override.load(Ordering::Relaxed)
    }

    pub fn set_glow_color_override(&self, color: i32) {
        self.glow_color_override.store(color, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::GLOW_COLOR_OVERRIDE,
            VarInt(color),
        );
    }

    #[allow(clippy::too_many_lines)]
    pub fn init_display_data_tracker(&self) {
        let view_range = *self
            .view_range
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shadow_radius = *self
            .shadow_radius
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shadow_strength = *self
            .shadow_strength
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let width = *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let height = *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let translation = *self
            .translation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let scale = *self
            .scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let left_rotation = *self
            .left_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let right_rotation = *self
            .right_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        self.entity.synched_data.set_forced(
            pumpkin_data::tracked_data::display::START_INTERPOLATION,
            VarInt(self.interpolation_start_delta_ticks.load(Ordering::Relaxed)),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::INTERPOLATION_DURATION,
            VarInt(self.interpolation_duration.load(Ordering::Relaxed)),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::TRANSLATION,
            Vector3fSerializer(translation.x, translation.y, translation.z),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::SCALE,
            Vector3fSerializer(scale.x, scale.y, scale.z),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::LEFT_ROTATION,
            QuaternionfSerializer(
                left_rotation[0],
                left_rotation[1],
                left_rotation[2],
                left_rotation[3],
            ),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::RIGHT_ROTATION,
            QuaternionfSerializer(
                right_rotation[0],
                right_rotation[1],
                right_rotation[2],
                right_rotation[3],
            ),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::BILLBOARD,
            self.billboard.load(Ordering::Relaxed),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::BRIGHTNESS,
            VarInt(self.brightness.load(Ordering::Relaxed)),
        );
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::VIEW_RANGE, view_range);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::SHADOW_RADIUS,
            shadow_radius,
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::SHADOW_STRENGTH,
            shadow_strength,
        );
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::WIDTH, width);
        self.entity
            .set_synced_data(pumpkin_data::tracked_data::display::HEIGHT, height);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::TELEPORT_DURATION,
            VarInt(self.teleport_duration.load(Ordering::Relaxed)),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::display::GLOW_COLOR_OVERRIDE,
            VarInt(self.glow_color_override.load(Ordering::Relaxed)),
        );
    }

    #[expect(clippy::too_many_lines)]
    pub fn write_display_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(
            "interpolation_duration",
            self.interpolation_duration.load(Ordering::Relaxed),
        );
        nbt.put_float(
            "view_range",
            *self
                .view_range
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_float(
            "shadow_radius",
            *self
                .shadow_radius
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_float(
            "shadow_strength",
            *self
                .shadow_strength
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_float(
            "width",
            *self
                .width
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_float(
            "height",
            *self
                .height
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_int(
            "teleport_duration",
            self.teleport_duration.load(Ordering::Relaxed),
        );

        let billboard_str = match self.billboard.load(Ordering::Relaxed) {
            1 => "vertical",
            2 => "horizontal",
            3 => "center",
            _ => "fixed",
        };
        nbt.put_string("billboard", billboard_str.into());

        let brightness = self.brightness.load(Ordering::Relaxed);
        if brightness != -1 {
            // `LightCoordsUtil.block` and `sky`.
            let block = (brightness >> 4) & MAX_LIGHT_LEVEL;
            let sky = (brightness >> 20) & MAX_LIGHT_LEVEL;
            let mut b_compound = NbtCompound::new();
            b_compound.put_int("block", block);
            b_compound.put_int("sky", sky);
            nbt.put("brightness", NbtTag::Compound(b_compound));
        }

        let glow_color_override = self.glow_color_override.load(Ordering::Relaxed);
        if glow_color_override != -1 {
            nbt.put_int("glow_color_override", glow_color_override);
        }

        let mut transform = NbtCompound::new();
        let trans = *self
            .translation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        transform.put(
            "translation",
            NbtTag::List(vec![trans.x.into(), trans.y.into(), trans.z.into()]),
        );
        let scale = *self
            .scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        transform.put(
            "scale",
            NbtTag::List(vec![scale.x.into(), scale.y.into(), scale.z.into()]),
        );
        let left_rot = *self
            .left_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        transform.put(
            "left_rotation",
            NbtTag::List(vec![
                left_rot[0].into(),
                left_rot[1].into(),
                left_rot[2].into(),
                left_rot[3].into(),
            ]),
        );
        let right_rot = *self
            .right_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        transform.put(
            "right_rotation",
            NbtTag::List(vec![
                right_rot[0].into(),
                right_rot[1].into(),
                right_rot[2].into(),
                right_rot[3].into(),
            ]),
        );
        nbt.put("transformation", NbtTag::Compound(transform));
    }

    /// Vanilla `Display.readAdditionalSaveData`: a missing or invalid field resets to its
    /// default, and the values are synced to clients like vanilla's `entityData.set`.
    pub fn read_display_nbt(&self, nbt: &NbtCompound) {
        let transformation = nbt
            .get("transformation")
            .and_then(Transformation::from_nbt)
            .unwrap_or(Transformation::IDENTITY);
        let [x, y, z] = transformation.translation;
        *self
            .translation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Vector3::new(x, y, z);
        *self
            .left_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = transformation.left_rotation;
        let [x, y, z] = transformation.scale;
        *self
            .scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Vector3::new(x, y, z);
        *self
            .right_rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = transformation.right_rotation;

        let int = |name: &str, default: i32| nbt.get_numeric_int(name).unwrap_or(default);
        let float = |name: &str, default: f32| nbt.get_numeric_float(name).unwrap_or(default);
        self.interpolation_duration
            .store(int("interpolation_duration", 0), Ordering::Relaxed);
        self.interpolation_start_delta_ticks
            .store(int("start_interpolation", 0), Ordering::Relaxed);
        self.teleport_duration.store(
            int("teleport_duration", 0).clamp(0, MAX_TELEPORT_DURATION),
            Ordering::Relaxed,
        );
        let billboard = match nbt.get_string("billboard") {
            Some("vertical") => 1,
            Some("horizontal") => 2,
            Some("center") => 3,
            _ => 0,
        };
        self.billboard.store(billboard, Ordering::Relaxed);
        for (field, name, default) in [
            (&self.view_range, "view_range", 1.0),
            (&self.shadow_radius, "shadow_radius", 0.0),
            (&self.shadow_strength, "shadow_strength", 1.0),
            (&self.width, "width", 0.0),
            (&self.height, "height", 0.0),
        ] {
            *field
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = float(name, default);
        }
        self.glow_color_override
            .store(int("glow_color_override", -1), Ordering::Relaxed);
        // `Brightness.CODEC`: both light levels in 0..=15, packed like `LightCoordsUtil.pack`.
        let light = |b: &NbtCompound, name: &str| {
            b.get_numeric_int(name)
                .filter(|level| (0..=MAX_LIGHT_LEVEL).contains(level))
        };
        let brightness = nbt
            .get_compound("brightness")
            .and_then(|b| Some(pack_brightness(light(b, "block")?, light(b, "sky")?)))
            .unwrap_or(-1);
        self.brightness.store(brightness, Ordering::Relaxed);

        // `setTransformationInterpolationDelay` forces the update, so an unchanged delay
        // still restarts the client's interpolation. Each display type syncs the rest with
        // `init_data_tracker` once its own fields are read.
        self.entity.set_synced_data_forced(
            pumpkin_data::tracked_data::display::START_INTERPOLATION,
            VarInt(self.interpolation_start_delta_ticks.load(Ordering::Relaxed)),
        );
    }
}

pub struct BlockDisplayEntity {
    pub display: DisplayEntity,
    pub block_state: AtomicI32,
}

impl BlockDisplayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `BlockDisplay.defineSynchedData`: air.
        entity.synched_data.define(
            pumpkin_data::tracked_data::block_display::DATA_BLOCK_STATE_ID,
            VarInt(0),
        );
        Arc::new(Self {
            display: DisplayEntity::new(entity),
            block_state: AtomicI32::new(0),
        })
    }

    pub fn get_block_state(&self) -> i32 {
        self.block_state.load(Ordering::Relaxed)
    }

    pub fn set_block_state(&self, block_state: i32) {
        self.block_state.store(block_state, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::block_display::BLOCK_STATE,
            VarInt(block_state),
        );
    }
}

impl EntityBase for BlockDisplayEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.display.write_display_nbt(nbt);
        let state = u16::try_from(self.block_state.load(Ordering::Relaxed)).unwrap_or(0);
        nbt.put(
            "block_state",
            PaletteEntry::state_to_nbt(BlockStateId::new_or_air(state)),
        );
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.display.read_display_nbt(nbt);
        // Older Pumpkin saves stored the raw state id. Unknown blocks fall back to air like vanilla.
        let state = match nbt.get("block_state") {
            Some(NbtTag::Int(id)) => Some(*id),
            Some(tag) => Some(i32::from(
                PaletteEntry::state_from_nbt(tag).map_or(0, BlockStateId::as_u16),
            )),
            None => None,
        };
        if let Some(state) = state {
            self.block_state.store(state, Ordering::Relaxed);
        }
        self.init_data_tracker();
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    fn init_data_tracker(&self) {
        self.display.init_display_data_tracker();
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::block_display::BLOCK_STATE,
            VarInt(self.block_state.load(Ordering::Relaxed)),
        );
    }

    fn get_entity(&self) -> &Entity {
        &self.display.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_ignored_by_pistons(&self) -> bool {
        true
    }

    fn can_hit(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }
}

pub struct ItemDisplayEntity {
    pub display: DisplayEntity,
    pub item_stack: Mutex<ItemStack>,
    pub item_display: AtomicU8,
}

impl ItemDisplayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `ItemDisplay.defineSynchedData`: no item, `ItemDisplayContext.NONE`.
        let data = &entity.synched_data;
        data.define(
            pumpkin_data::tracked_data::item_display::DATA_ITEM_STACK_ID,
            ItemStackSerializer::from(ItemStack::EMPTY.clone()),
        );
        data.define(
            pumpkin_data::tracked_data::item_display::DATA_ITEM_DISPLAY_ID,
            0u8,
        );
        Arc::new(Self {
            display: DisplayEntity::new(entity),
            item_stack: Mutex::new(ItemStack::new(0, &pumpkin_data::item::Item::AIR)),
            item_display: AtomicU8::new(0),
        })
    }

    pub fn get_item(&self) -> ItemStack {
        self.item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_item(&self, item: ItemStack) {
        *self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = item.clone();
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::item_display::ITEM,
            ItemStackSerializer::from(item),
        );
    }

    pub fn get_item_display_mode(&self) -> u8 {
        self.item_display.load(Ordering::Relaxed)
    }

    pub fn set_item_display_mode(&self, mode: u8) {
        self.item_display.store(mode, Ordering::Relaxed);
        self.display
            .entity
            .set_synced_data(pumpkin_data::tracked_data::item_display::ITEM_DISPLAY, mode);
    }
}

impl EntityBase for ItemDisplayEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.display.write_display_nbt(nbt);
        let item = self
            .item_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !item.is_empty() {
            let mut item_compound = NbtCompound::new();
            item.write_item_stack(&mut item_compound);
            nbt.put_compound("item", item_compound);
        }
        drop(item);
        let display_mode_str = match self.item_display.load(Ordering::Relaxed) {
            1 => "thirdperson_lefthand",
            2 => "thirdperson_righthand",
            3 => "firstperson_lefthand",
            4 => "firstperson_righthand",
            5 => "head",
            6 => "gui",
            7 => "ground",
            8 => "fixed",
            _ => "none",
        };
        nbt.put_string("item_display", display_mode_str.to_string());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.display.read_display_nbt(nbt);
        if let Some(item) = nbt
            .get_compound("item")
            .and_then(ItemStack::read_item_stack)
        {
            *self
                .item_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = item;
        }
        if let Some(mode_str) = nbt.get_string("item_display") {
            let mode = match mode_str {
                "thirdperson_lefthand" => 1,
                "thirdperson_righthand" => 2,
                "firstperson_lefthand" => 3,
                "firstperson_righthand" => 4,
                "head" => 5,
                "gui" => 6,
                "ground" => 7,
                "fixed" => 8,
                _ => 0,
            };
            self.item_display.store(mode, Ordering::Relaxed);
        }
        self.init_data_tracker();
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    fn init_data_tracker(&self) {
        self.display.init_display_data_tracker();
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::item_display::ITEM,
            ItemStackSerializer::from(
                self.item_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        );
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::item_display::ITEM_DISPLAY,
            self.item_display.load(Ordering::Relaxed),
        );
    }

    fn get_entity(&self) -> &Entity {
        &self.display.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_ignored_by_pistons(&self) -> bool {
        true
    }

    fn can_hit(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }
}

pub struct TextDisplayEntity {
    pub display: DisplayEntity,
    pub text: Mutex<TextComponent>,
    /// Text read from NBT, waiting for `read_nbt_as_source` to resolve it.
    loaded_text: Mutex<Option<TextComponent>>,
    pub line_width: AtomicI32,
    pub background: AtomicI32,
    pub text_opacity: AtomicI8,
    pub flags: AtomicU8,
}

impl TextDisplayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `TextDisplay.defineSynchedData`.
        let data = &entity.synched_data;
        data.define(
            pumpkin_data::tracked_data::text_display::DATA_TEXT_ID,
            TextComponent::text(""),
        );
        data.define(
            pumpkin_data::tracked_data::text_display::DATA_LINE_WIDTH_ID,
            VarInt(200),
        );
        data.define(
            pumpkin_data::tracked_data::text_display::DATA_BACKGROUND_COLOR_ID,
            VarInt(0x4000_0000),
        );
        data.define(
            pumpkin_data::tracked_data::text_display::DATA_TEXT_OPACITY_ID,
            -1i8,
        );
        data.define(
            pumpkin_data::tracked_data::text_display::DATA_STYLE_FLAGS_ID,
            0u8,
        );
        Arc::new(Self {
            display: DisplayEntity::new(entity),
            text: Mutex::new(TextComponent::text("")),
            loaded_text: Mutex::new(None),
            line_width: AtomicI32::new(200),
            background: AtomicI32::new(1_073_741_824),
            text_opacity: AtomicI8::new(-1),
            flags: AtomicU8::new(0),
        })
    }

    pub fn get_text(&self) -> TextComponent {
        self.text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_text(&self, text: TextComponent) {
        *self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = text.clone();
        self.display
            .entity
            .set_synced_data(pumpkin_data::tracked_data::text_display::TEXT, text);
    }

    pub fn get_line_width(&self) -> i32 {
        self.line_width.load(Ordering::Relaxed)
    }

    pub fn set_line_width(&self, width: i32) {
        self.line_width.store(width, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::LINE_WIDTH,
            VarInt(width),
        );
    }

    pub fn get_background_color(&self) -> i32 {
        self.background.load(Ordering::Relaxed)
    }

    pub fn set_background_color(&self, color: i32) {
        self.background.store(color, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::BACKGROUND,
            VarInt(color),
        );
    }

    pub fn get_text_opacity(&self) -> i8 {
        self.text_opacity.load(Ordering::Relaxed)
    }

    pub fn set_text_opacity(&self, opacity: i8) {
        self.text_opacity.store(opacity, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_OPACITY,
            opacity as u8,
        );
    }

    pub fn get_shadow(&self) -> bool {
        (self.flags.load(Ordering::Relaxed) & 1) != 0
    }

    pub fn set_shadow(&self, shadow: bool) {
        let mut flags = self.flags.load(Ordering::Relaxed);
        if shadow {
            flags |= 1;
        } else {
            flags &= !1;
        }
        self.flags.store(flags, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_DISPLAY_FLAGS,
            flags,
        );
    }

    pub fn get_see_through(&self) -> bool {
        (self.flags.load(Ordering::Relaxed) & 2) != 0
    }

    pub fn set_see_through(&self, see_through: bool) {
        let mut flags = self.flags.load(Ordering::Relaxed);
        if see_through {
            flags |= 2;
        } else {
            flags &= !2;
        }
        self.flags.store(flags, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_DISPLAY_FLAGS,
            flags,
        );
    }

    pub fn get_use_default_background(&self) -> bool {
        (self.flags.load(Ordering::Relaxed) & 4) != 0
    }

    pub fn set_use_default_background(&self, default_bg: bool) {
        let mut flags = self.flags.load(Ordering::Relaxed);
        if default_bg {
            flags |= 4;
        } else {
            flags &= !4;
        }
        self.flags.store(flags, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_DISPLAY_FLAGS,
            flags,
        );
    }

    pub fn get_alignment(&self) -> u8 {
        let flags = self.flags.load(Ordering::Relaxed);
        if flags & 8 != 0 {
            1
        } else if flags & 16 != 0 {
            2
        } else {
            0
        }
    }

    pub fn set_alignment(&self, align: u8) {
        let mut flags = self.flags.load(Ordering::Relaxed) & !0b1_1000;
        if align == 1 {
            flags |= 8;
        } else if align == 2 {
            flags |= 16;
        }
        self.flags.store(flags, Ordering::Relaxed);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_DISPLAY_FLAGS,
            flags,
        );
    }
}

impl EntityBase for TextDisplayEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        self.display.write_display_nbt(nbt);
        nbt.put(
            "text",
            self.text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0
                .to_nbt_tag(),
        );
        nbt.put_int("line_width", self.line_width.load(Ordering::Relaxed));
        nbt.put_int("background", self.background.load(Ordering::Relaxed));
        nbt.put_byte("text_opacity", self.text_opacity.load(Ordering::Relaxed));

        let flags = self.flags.load(Ordering::Relaxed);
        nbt.put_bool("shadow", flags & 1 != 0);
        nbt.put_bool("see_through", flags & 2 != 0);
        nbt.put_bool("default_background", flags & 4 != 0);
        let align_str = if flags & 8 != 0 {
            "left"
        } else if flags & 16 != 0 {
            "right"
        } else {
            "center"
        };
        nbt.put_string("alignment", align_str.to_string());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.display.read_display_nbt(nbt);
        if let Some(text) = nbt.get("text") {
            // Older Pumpkin saves stored the component as a JSON string.
            let legacy_json = text
                .extract_string()
                .filter(|s| s.starts_with('{'))
                .and_then(|s| pumpkin_util::serde_json::from_str(s).ok());
            // Resolved in `read_nbt_as_source`.
            *self
                .loaded_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(legacy_json.unwrap_or_else(|| TextComponent::from_nbt(text)));
        }
        if let Some(lw) = nbt.get_int("line_width") {
            self.line_width.store(lw, Ordering::Relaxed);
        }
        if let Some(bg) = nbt.get_int("background") {
            self.background.store(bg, Ordering::Relaxed);
        }
        if let Some(opacity) = nbt.get_byte("text_opacity") {
            self.text_opacity.store(opacity, Ordering::Relaxed);
        }

        let mut flags = 0u8;
        if nbt.get_bool("shadow").unwrap_or(false) {
            flags |= 1;
        }
        if nbt.get_bool("see_through").unwrap_or(false) {
            flags |= 2;
        }
        if nbt.get_bool("default_background").unwrap_or(false) {
            flags |= 4;
        }
        if let Some(align) = nbt.get_string("alignment") {
            match align {
                "left" => flags |= 8,
                "right" => flags |= 16,
                _ => {}
            }
        }
        self.flags.store(flags, Ordering::Relaxed);
        self.init_data_tracker();
    }

    /// Vanilla `TextDisplay.readAdditionalSaveData`: loaded text is resolved for the display
    /// with gamemaster permissions; text that fails to resolve isn't set.
    fn read_nbt_as_source(&self, this: &Arc<dyn EntityBase>) {
        let Some(text) = self
            .loaded_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let entity = &self.display.entity;
        let world = entity.world.load_full();
        let Some(server) = world.server.upgrade() else {
            self.set_text(TextComponent::empty());
            return;
        };
        // `createCommandSourceStackForNameResolution`; `Dummy` has the gamemaster level.
        let source = CommandSource::new(
            CommandSender::Dummy,
            world,
            None,
            entity.pos.load(),
            Vector2::new(entity.pitch.load(), entity.yaw.load()),
            String::new(),
            TextComponent::empty(),
            server,
        )
        .with_entity(this.clone());
        if let Ok(resolved) = text_resolution::resolve(&source, &text) {
            self.set_text(resolved);
        } else {
            warn!("Failed to parse display entity text {text:?}");
        }
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    fn init_data_tracker(&self) {
        self.display.init_display_data_tracker();
        let text = self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.display
            .entity
            .set_synced_data(pumpkin_data::tracked_data::text_display::TEXT, text);
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::LINE_WIDTH,
            VarInt(self.line_width.load(Ordering::Relaxed)),
        );
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::BACKGROUND,
            VarInt(self.background.load(Ordering::Relaxed)),
        );
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_OPACITY,
            self.text_opacity.load(Ordering::Relaxed) as u8,
        );
        self.display.entity.set_synced_data(
            pumpkin_data::tracked_data::text_display::TEXT_DISPLAY_FLAGS,
            self.flags.load(Ordering::Relaxed),
        );
    }

    fn get_entity(&self) -> &Entity {
        &self.display.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn is_ignored_by_pistons(&self) -> bool {
        true
    }

    fn can_hit(&self) -> bool {
        false
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        false
    }
}
