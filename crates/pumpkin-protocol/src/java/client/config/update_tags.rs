use std::borrow::Cow;
use std::io::Write;

use crate::{ClientPacket, WritingError, ser::NetworkWriteExt};

use crate::codec::var_int::VarInt;
use pumpkin_data::packet::clientbound::config::UPDATE_TAGS;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

/// The tags of one registry, by tag id with the registry ids of their elements (vanilla
/// `TagNetworkSerialization.NetworkPayload`).
#[derive(Clone, Debug)]
pub struct RegistryTags {
    /// The registry's id, e.g. `minecraft:item`.
    pub registry: Cow<'static, str>,
    pub tags: Vec<(Cow<'static, str>, Cow<'static, [u16]>)>,
}

#[java_packet(UPDATE_TAGS)]
pub struct CUpdateTags<'a> {
    pub registries: &'a [RegistryTags],
}

impl<'a> CUpdateTags<'a> {
    #[must_use]
    pub const fn new(registries: &'a [RegistryTags]) -> Self {
        Self { registries }
    }
}

impl ClientPacket for CUpdateTags<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write_tag_registries(write, self.registries)
    }
}

/// Writes the tags of one registry: the tag count, then each tag id with its element ids.
pub fn write_registry_tags(
    mut write: impl Write,
    registry: &RegistryTags,
) -> Result<(), WritingError> {
    let count = registry.tags.len();
    write.write_var_int(&count.try_into().map_err(|_| {
        WritingError::Message(format!("{count} isn't representable as a VarInt"))
    })?)?;
    for (tag, ids) in &registry.tags {
        // This is technically a `ResourceLocation` but same thing
        write.write_string_bounded(tag, u16::MAX as usize)?;
        write.write_list(ids, |p, &id| p.write_var_int(&VarInt::from(id)))?;
    }
    Ok(())
}

/// Writes the registries of `ClientboundUpdateTagsPacket`, for both its configuration and its
/// play variant.
pub fn write_tag_registries(
    mut write: impl Write,
    registries: &[RegistryTags],
) -> Result<(), WritingError> {
    write.write_list(registries, |p, registry| {
        p.write_string(&registry.registry)?;
        write_registry_tags(p, registry)
    })
}
