use std::io::Write;

use crate::packet::MultiVersionJavaPacket;
use crate::{ClientPacket, WritingError, ser::NetworkWriteExt};

use crate::codec::var_int::VarInt;
use crate::java::client::config::{RegistryTags, write_registry_tags, write_tag_registries};
use pumpkin_data::packet::clientbound::play::UPDATE_TAGS;
use pumpkin_util::version::JavaMinecraftVersion;

pub struct CUpdateTagsPlay<'a> {
    pub registries: &'a [RegistryTags],
}

impl MultiVersionJavaPacket for CUpdateTagsPlay<'_> {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        let id = UPDATE_TAGS.to_id(version);
        if id != -1 {
            return id;
        }
        #[allow(clippy::match_same_arms)]
        match version {
            JavaMinecraftVersion::V_1_20_5 => 0x78,
            JavaMinecraftVersion::V_1_20_3 => 0x74,
            JavaMinecraftVersion::V_1_20_2 => 0x70,
            JavaMinecraftVersion::V_1_20 | JavaMinecraftVersion::V_1_19_4 => 0x6E,
            JavaMinecraftVersion::V_1_19_3 => 0x6A,
            JavaMinecraftVersion::V_1_19_1 => 0x6B,
            JavaMinecraftVersion::V_1_19 => 0x68,
            JavaMinecraftVersion::V_1_18_2 | JavaMinecraftVersion::V_1_18 => 0x67,
            JavaMinecraftVersion::V_1_17_1 | JavaMinecraftVersion::V_1_17 => 0x66,
            JavaMinecraftVersion::V_1_16_4
            | JavaMinecraftVersion::V_1_16_3
            | JavaMinecraftVersion::V_1_16_2
            | JavaMinecraftVersion::V_1_16_1
            | JavaMinecraftVersion::V_1_16
            | JavaMinecraftVersion::V_1_14_4
            | JavaMinecraftVersion::V_1_14_3
            | JavaMinecraftVersion::V_1_14_2
            | JavaMinecraftVersion::V_1_14_1
            | JavaMinecraftVersion::V_1_14 => 0x5B,
            JavaMinecraftVersion::V_1_15_2
            | JavaMinecraftVersion::V_1_15_1
            | JavaMinecraftVersion::V_1_15 => 0x5C,
            JavaMinecraftVersion::V_1_13_2
            | JavaMinecraftVersion::V_1_13_1
            | JavaMinecraftVersion::V_1_13 => 0x55,
            _ => -1,
        }
    }
}

impl<'a> CUpdateTagsPlay<'a> {
    #[must_use]
    pub const fn new(registries: &'a [RegistryTags]) -> Self {
        Self { registries }
    }
}

impl ClientPacket for CUpdateTagsPlay<'_> {
    fn write_packet_data(
        &self,
        mut write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        if version < &JavaMinecraftVersion::V_1_13 {
            return Ok(());
        }

        if version < &JavaMinecraftVersion::V_1_17 {
            // 1.13 to 1.16.5 fixed category format without registry identifier strings:
            // 1.13 - 1.13.2: 3 categories (Block, Item, Fluid)
            // 1.14 - 1.16.5: 4 categories (Block, Item, Fluid, EntityType)
            let categories = if version < &JavaMinecraftVersion::V_1_14 {
                &["minecraft:block", "minecraft:item", "minecraft:fluid"][..]
            } else {
                &[
                    "minecraft:block",
                    "minecraft:item",
                    "minecraft:fluid",
                    "minecraft:entity_type",
                ][..]
            };

            for &category in categories {
                match self.registries.iter().find(|r| r.registry == category) {
                    Some(registry) => write_registry_tags(&mut write, registry)?,
                    None => write.write_var_int(&VarInt::from(0))?,
                }
            }
            return Ok(());
        }

        write_tag_registries(write, self.registries)
    }
}
