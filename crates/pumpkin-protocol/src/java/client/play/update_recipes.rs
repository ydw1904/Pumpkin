use std::io::Write;

use crate::{
    ClientPacket, ServerPacket, VarInt,
    codec::item_stack_seralizer::ItemStackTemplateSerializer,
    codec::recipe::{
        ItemHolderSet, RecipePropertySet, RecipePropertySetKey, SelectableStonecutterRecipe,
    },
    ser::{NetworkReadSliceExt, NetworkWriteExt, ReadingError, WritingError},
};
use pumpkin_data::packet::clientbound::play::UPDATE_RECIPES;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

use super::recipe_book_add::SLOT_DISPLAY_ITEM_STACK;

#[java_packet(UPDATE_RECIPES)]
pub struct CUpdateRecipes<'a> {
    pub raw_data: &'a [u8],
}

impl<'a> CUpdateRecipes<'a> {
    #[must_use]
    pub const fn new(raw_data: &'a [u8]) -> Self {
        Self { raw_data }
    }
}

impl ClientPacket for CUpdateRecipes<'_> {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write.write_slice(self.raw_data)?;
        Ok(())
    }
}

impl<'a> ServerPacket<'a> for CUpdateRecipes<'a> {
    fn read(bytebuf: &mut &'a [u8], _version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let raw_data = bytebuf.read_remaining_slice_borrowed(usize::MAX)?;
        Ok(Self { raw_data })
    }
}

/// Writes the data of `ClientboundUpdateRecipesPacket` (1.21.2 and later): the recipe property
/// sets, then the stonecutter recipes with `SelectableRecipe.SingleInputSet.noRecipeCodec`.
pub fn write_update_recipes(
    write: &mut impl Write,
    item_sets: &[(RecipePropertySetKey, RecipePropertySet)],
    stonecutter_recipes: &[SelectableStonecutterRecipe],
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(item_sets.len() as i32))?;
    for (key, set) in item_sets {
        write.write_string(key.id())?;
        write.write_var_int(&VarInt(set.item_ids().len() as i32))?;
        for id in set.item_ids() {
            write.write_var_int(&VarInt(i32::from(id)))?;
        }
    }
    write.write_var_int(&VarInt(stonecutter_recipes.len() as i32))?;
    for recipe in stonecutter_recipes {
        write_item_holder_set(write, &recipe.input)?;
        // `StonecutterRecipe.resultDisplay`: a `SlotDisplay.ItemStackSlotDisplay`.
        write.write_var_int(&VarInt(SLOT_DISPLAY_ITEM_STACK as i32))?;
        ItemStackTemplateSerializer(std::borrow::Cow::Borrowed(&recipe.result))
            .write_with_version(write, &version)?;
    }
    Ok(())
}

/// `ByteBufCodecs.holderSet`: 0 and the tag id, or the item count plus one and the items.
fn write_item_holder_set(write: &mut impl Write, set: &ItemHolderSet) -> Result<(), WritingError> {
    match set {
        ItemHolderSet::Named { tag, .. } => {
            write.write_var_int(&VarInt(0))?;
            write.write_string(tag)
        }
        ItemHolderSet::Direct(items) => {
            write.write_var_int(&VarInt(items.len() as i32 + 1))?;
            for id in items {
                write.write_var_int(&VarInt(i32::from(*id)))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_data::recipes::RecipeIngredientTypes;

    use super::*;

    /// The bytes vanilla's `ClientboundUpdateRecipesPacket.STREAM_CODEC` writes for one property
    /// set and two stonecutter recipes, one with a tag input and one with an item.
    #[test]
    fn writes_vanilla_stream_codec_layout() {
        for item in [&Item::STONE, &Item::GRANITE, &Item::POLISHED_GRANITE] {
            assert!(
                item.id < 0x80,
                "the expected bytes hold single-byte var ints"
            );
        }
        let mut furnace_input = RecipePropertySet::default();
        furnace_input.add_item(&Item::GRANITE);
        let tagged =
            ItemHolderSet::from_builtin(&RecipeIngredientTypes::Tagged("#minecraft:logs")).unwrap();
        let direct =
            ItemHolderSet::from_builtin(&RecipeIngredientTypes::Simple("minecraft:stone")).unwrap();
        let recipes = [
            SelectableStonecutterRecipe {
                recipe_id: Cow::Borrowed("minecraft:a"),
                input: tagged,
                result: ItemStack::new(1, &Item::GRANITE),
            },
            SelectableStonecutterRecipe {
                recipe_id: Cow::Borrowed("minecraft:b"),
                input: direct,
                result: ItemStack::new(2, &Item::POLISHED_GRANITE),
            },
        ];
        let mut bytes = Vec::new();
        write_update_recipes(
            &mut bytes,
            &[(RecipePropertySetKey::FurnaceInput, furnace_input)],
            &recipes,
            JavaMinecraftVersion::V_26_3,
        )
        .unwrap();

        let mut expected = vec![1, 23];
        expected.extend_from_slice(b"minecraft:furnace_input");
        expected.extend_from_slice(&[1, Item::GRANITE.id as u8]);
        expected.push(2);
        // A named holder set: 0, then the tag id.
        expected.extend_from_slice(&[0, 14]);
        expected.extend_from_slice(b"minecraft:logs");
        // `SlotDisplay.ItemStackSlotDisplay`: the item, the count and an empty patch.
        expected.extend_from_slice(&[5, Item::GRANITE.id as u8, 1, 0, 0]);
        // A direct holder set: the size plus one, then the items.
        expected.extend_from_slice(&[2, Item::STONE.id as u8]);
        expected.extend_from_slice(&[5, Item::POLISHED_GRANITE.id as u8, 2, 0, 0]);
        assert_eq!(bytes, expected);
    }
}
