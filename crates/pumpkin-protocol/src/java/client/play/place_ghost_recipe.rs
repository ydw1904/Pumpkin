use pumpkin_data::packet::clientbound::play::PLACE_GHOST_RECIPE;
use pumpkin_macros::java_packet;

use crate::{ClientPacket, ser::NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

#[java_packet(PLACE_GHOST_RECIPE)]
pub struct CPlaceGhostRecipe<'a> {
    pub window_id: u8,
    pub recipe_id: &'a str,
}

impl<'a> CPlaceGhostRecipe<'a> {
    #[must_use]
    pub const fn new(window_id: u8, recipe_id: &'a str) -> Self {
        Self {
            window_id,
            recipe_id,
        }
    }
}

impl ClientPacket for CPlaceGhostRecipe<'_> {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_container_id(&crate::VarInt(i32::from(self.window_id)), version)?;
        write.write_string(self.recipe_id)?;
        Ok(())
    }
}

/// `ClientboundPlaceGhostRecipePacket` as vanilla writes it, with the full recipe display of a
/// built-in recipe. Kept apart from [`CPlaceGhostRecipe`] so the plugin packet bindings stay as
/// they are.
pub struct CPlaceGhostRecipeDisplay {
    pub window_id: u8,
    pub recipe: crate::java::client::play::BuiltinRecipeDisplay,
}

impl crate::packet::MultiVersionJavaPacket for CPlaceGhostRecipeDisplay {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        PLACE_GHOST_RECIPE.to_id(version)
    }
}

impl ClientPacket for CPlaceGhostRecipeDisplay {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), crate::ser::WritingError> {
        write.write_container_id(&crate::VarInt(i32::from(self.window_id)), version)?;
        self.recipe.write_display(&mut write, *version)
    }
}
