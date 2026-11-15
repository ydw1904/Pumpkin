use super::BlockEntity;
use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;
use std::sync::Mutex;

pub struct EnchantingTableBlockEntity {
    pub position: BlockPos,
    pub custom_name: Mutex<Option<TextComponent>>,
}

impl BlockEntity for EnchantingTableBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let custom_name = nbt.get("CustomName").map(TextComponent::from_nbt);
        Self {
            position,
            custom_name: Mutex::new(custom_name),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Ok(name) = self.custom_name.lock()
            && let Some(name) = name.as_ref()
        {
            nbt.put("CustomName", name.0.to_nbt_tag());
        }
    }

    fn bedrock_block_actor_data(
        &self,
        _state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let custom_name = self
            .custom_name
            .try_lock()
            .ok()
            .and_then(|g| g.clone())
            .map(TextComponent::get_text);
        Some(
            super::bedrock::BedrockBlockActorBuilder::new("EnchantTable", self.position)
                .movable(false)
                .custom_name(custom_name.as_deref())
                .put_float("rott", 0.0)
                .finish(),
        )
    }

    fn custom_name(&self) -> Option<TextComponent> {
        self.custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn collect_item_components(&self, stack: &mut ItemStack) {
        if let Some(name) = BlockEntity::custom_name(self) {
            stack.set_data_component(CustomNameImpl { name });
        }
    }

    fn apply_item_components(&self, stack: &ItemStack) {
        *self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stack
            .get_data_component::<CustomNameImpl>()
            .map(|custom_name| custom_name.name.clone());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl EnchantingTableBlockEntity {
    pub const ID: &'static str = "minecraft:enchanting_table";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            custom_name: Mutex::new(None),
        }
    }
}
