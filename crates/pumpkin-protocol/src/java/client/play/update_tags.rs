use std::io::Write;

use crate::packet::MultiVersionJavaPacket;
use crate::{ClientPacket, WritingError, ser::NetworkWriteExt};

use crate::codec::var_int::VarInt;
use crate::java::client::config::write_tag_registries;
use pumpkin_data::{
    packet::clientbound::play::UPDATE_TAGS,
    tag::{RegistryKey, get_registry_key_tags},
};
use pumpkin_util::version::JavaMinecraftVersion;

pub struct CUpdateTagsPlay<'a> {
    pub tags: &'a [pumpkin_data::tag::RegistryKey],
    /// Block tags from datapacks, like [`crate::java::client::config::CUpdateTags::block_tags`].
    pub block_tags: &'a [(String, Vec<u16>)],
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
    pub const fn new(tags: &'a [RegistryKey]) -> Self {
        Self {
            tags,
            block_tags: &[],
        }
    }

    #[must_use]
    pub const fn with_block_tags(mut self, block_tags: &'a [(String, Vec<u16>)]) -> Self {
        self.block_tags = block_tags;
        self
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
                &[RegistryKey::Block, RegistryKey::Item, RegistryKey::Fluid][..]
            } else {
                &[
                    RegistryKey::Block,
                    RegistryKey::Item,
                    RegistryKey::Fluid,
                    RegistryKey::EntityType,
                ][..]
            };

            for &key in categories {
                let Some(values) = get_registry_key_tags(*version, key) else {
                    write.write_var_int(&VarInt::from(0))?;
                    continue;
                };
                write.write_var_int(&VarInt(values.len() as i32))?;
                for (tag_name, tag_val) in values.entries() {
                    write.write_string_bounded(tag_name, u16::MAX as usize)?;
                    write.write_list(tag_val.1, |p, &id| p.write_var_int(&VarInt::from(id)))?;
                }
            }
            return Ok(());
        }

        write_tag_registries(write, self.tags, self.block_tags, *version)
    }
}
