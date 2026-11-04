use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::translation;
use pumpkin_inventory::Inventory;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::hover::HoverEvent;

use crate::block::entities::BlockEntity;
use crate::command::argument_builder::{
    ArgumentBuilder, RequiredArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::argument_type::JavaClientArgumentType;
use crate::command::argument_types::client_parsed::ClientParsed;
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::item::ItemStackArgumentType;
use crate::command::argument_types::loot_resource::{
    LootRegistry, LootResource, LootResourceArgumentType,
};
use crate::command::argument_types::slot::{SlotArgumentType, SlotsArgumentType};
use crate::command::commands::execute::entity_slot_items;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::EntityBase;
use crate::world::loot::{LootContextParameters, apply_item_modifier};

const DESCRIPTION: &str = "Modifies items in block or entity inventories.";
const PERMISSION: &str = "minecraft:command.item";

const ERROR_NOT_CONTAINER: CommandErrorType<3> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_TARGET_NOT_A_CONTAINER,
    translation::java::COMMANDS_ITEM_TARGET_NOT_A_CONTAINER,
);

const ERROR_NO_SUCH_SLOT: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_TARGET_NO_SUCH_SLOT,
    translation::java::COMMANDS_ITEM_TARGET_NO_SUCH_SLOT,
);

const ERROR_STACK_TOO_BIG: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENTS_ITEM_OVERSTACKED,
    translation::java::ARGUMENTS_ITEM_OVERSTACKED,
);

/// `ItemInput.createItemStack`: a count above the item's stack size is an error.
fn check_stack_size(stack: &ItemStack, count: i32) -> Result<(), CommandSyntaxError> {
    let max = stack.get_max_stack_size();
    if count > i32::from(max) {
        return Err(ERROR_STACK_TOO_BIG.create_without_context(
            TextComponent::text(format!("minecraft:{}", stack.item.registry_key)),
            TextComponent::text(max.to_string()),
        ));
    }
    Ok(())
}

struct BlockReplaceExecutor {
    has_count: bool,
}

impl CommandExecutor for BlockReplaceExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.source.world();
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "target")?;
        let slot = SlotArgumentType::get(context, "slots")?;
        let parsed_stack = ItemStackArgumentType::get(context, "item")?;
        let item = parsed_stack.item;
        let count = if self.has_count {
            IntegerArgumentType::get(context, "count")?
        } else {
            1
        };
        check_stack_size(&parsed_stack, count)?;

        let block_entity = world.get_block_entity(&pos).ok_or_else(|| {
            ERROR_NOT_CONTAINER.create_without_context(
                TextComponent::text(pos.0.x.to_string()),
                TextComponent::text(pos.0.y.to_string()),
                TextComponent::text(pos.0.z.to_string()),
            )
        })?;

        let inventory = block_entity.get_inventory().ok_or_else(|| {
            ERROR_NOT_CONTAINER.create_without_context(
                TextComponent::text(pos.0.x.to_string()),
                TextComponent::text(pos.0.y.to_string()),
                TextComponent::text(pos.0.z.to_string()),
            )
        })?;

        if slot >= inventory.size() {
            return Err(
                ERROR_NO_SUCH_SLOT.create_without_context(TextComponent::text(slot.to_string()))
            );
        }

        let mut item_stack = parsed_stack.clone();
        item_stack.item_count = count as u8;
        inventory.set_stack(slot, item_stack);

        let item_name = item.registry_key;
        let msg = TextComponent::translate_cross(
            translation::java::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS_KNOWN_ITEM,
            translation::java::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS_KNOWN_ITEM,
            [
                TextComponent::text("1"),
                TextComponent::text(pos.0.x.to_string()),
                TextComponent::text(pos.0.y.to_string()),
                TextComponent::text(pos.0.z.to_string()),
                TextComponent::text("[")
                    .add_child(item.translated_name())
                    .add_child(TextComponent::text("]"))
                    .hover_event(HoverEvent::ShowItem {
                        id: item_name.to_string().into(),
                        count: Some(count),
                        components: None,
                    }),
            ],
        );
        context.source.send_feedback(msg, true);

        Ok(1)
    }
}

/// Vanilla `Entity.getSlot(slot).set(stack)` for the slots Pumpkin models; false when the entity
/// has no such slot.
fn set_entity_slot(target: &dyn EntityBase, mojang_slot: usize, item_stack: &ItemStack) -> bool {
    if let Some(player) = target.get_player() {
        if (200..=226).contains(&mojang_slot) {
            let ender_slot = mojang_slot - 200;
            if ender_slot < player.ender_chest_inventory.size() {
                player
                    .ender_chest_inventory
                    .set_stack(ender_slot, item_stack.clone());
                return true;
            }
            return false;
        }
        let inventory = player.inventory();
        let mapped_slot = match mojang_slot {
            98 => Some(inventory.get_selected_slot() as usize),
            99 => Some(40),
            100 => Some(36),
            101 => Some(37),
            102 => Some(38),
            103 => Some(39),
            0..=35 => Some(mojang_slot),
            _ => None,
        };
        let Some(slot) = mapped_slot.filter(|&slot| slot < inventory.size()) else {
            return false;
        };
        inventory.set_stack(slot, item_stack.clone());
        let eq_slot = match slot {
            36 => Some(EquipmentSlot::FEET),
            37 => Some(EquipmentSlot::LEGS),
            38 => Some(EquipmentSlot::CHEST),
            39 => Some(EquipmentSlot::HEAD),
            40 => Some(EquipmentSlot::OFF_HAND),
            _ if slot == inventory.get_selected_slot() as usize => Some(EquipmentSlot::MAIN_HAND),
            _ => None,
        };
        if let Some(eq) = eq_slot {
            player
                .living_entity
                .send_equipment_changes(&[(eq, item_stack.clone())]);
        }
        return true;
    }
    if let Some(living) = target.get_living_entity() {
        let mapped_eq = match mojang_slot {
            98 => Some(EquipmentSlot::MAIN_HAND),
            99 => Some(EquipmentSlot::OFF_HAND),
            100 => Some(EquipmentSlot::FEET),
            101 => Some(EquipmentSlot::LEGS),
            102 => Some(EquipmentSlot::CHEST),
            103 => Some(EquipmentSlot::HEAD),
            105 => Some(EquipmentSlot::BODY),
            106 => Some(EquipmentSlot::SADDLE),
            _ => None,
        };
        if let Some(eq) = mapped_eq {
            living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .put(&eq, item_stack.clone());
            living.send_equipment_changes(&[(eq, item_stack.clone())]);
            return true;
        }
    }
    false
}

struct EntityReplaceExecutor {
    has_count: bool,
}

impl CommandExecutor for EntityReplaceExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let targets = EntityArgumentType::get_entities(context, "target")?;
        let mojang_slot = SlotArgumentType::get(context, "slots")?;
        let parsed_stack = ItemStackArgumentType::get(context, "item")?;
        let item = parsed_stack.item;
        let count = if self.has_count {
            IntegerArgumentType::get(context, "count")?
        } else {
            1
        };
        check_stack_size(&parsed_stack, count)?;

        let mut modified_count = 0;
        let mut item_stack = parsed_stack.clone();
        item_stack.item_count = count as u8;

        for target in &targets {
            if set_entity_slot(target.as_ref(), mojang_slot, &item_stack) {
                modified_count += 1;
            }
        }

        if modified_count == 0 {
            return Err(ERROR_NO_SUCH_SLOT
                .create_without_context(TextComponent::text(mojang_slot.to_string())));
        }

        let item_name = item.registry_key;
        let msg = if targets.len() == 1 {
            TextComponent::translate_cross(
                translation::java::COMMANDS_ITEM_ENTITY_SET_SUCCESS_SINGLE,
                translation::java::COMMANDS_ITEM_ENTITY_SET_SUCCESS_SINGLE,
                [
                    targets[0].as_ref().get_display_name(),
                    TextComponent::text("[")
                        .add_child(item.translated_name())
                        .add_child(TextComponent::text("]"))
                        .hover_event(HoverEvent::ShowItem {
                            id: item_name.to_string().into(),
                            count: Some(count),
                            components: None,
                        }),
                ],
            )
        } else {
            TextComponent::translate_cross(
                translation::java::COMMANDS_ITEM_ENTITY_SET_SUCCESS_MULTIPLE,
                translation::java::COMMANDS_ITEM_ENTITY_SET_SUCCESS_MULTIPLE,
                [
                    TextComponent::text(modified_count.to_string()),
                    TextComponent::text("[")
                        .add_child(item.translated_name())
                        .add_child(TextComponent::text("]"))
                        .hover_event(HoverEvent::ShowItem {
                            id: item_name.to_string().into(),
                            count: Some(count),
                            components: None,
                        }),
                ],
            )
        };
        context.source.send_feedback(msg, true);

        Ok(modified_count)
    }
}

const ERROR_NO_CHANGES: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_TARGET_FAILED,
    translation::java::COMMANDS_ITEM_TARGET_FAILED,
);

const ERROR_NO_SUCH_MODIFIER: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_OR_ID_NO_SUCH_ELEMENT,
    translation::java::ARGUMENT_RESOURCE_OR_ID_NO_SUCH_ELEMENT,
);

/// Vanilla `ItemCommands.modifyItems` on a block or the entities of a selector.
struct ModifyExecutor {
    block: bool,
}

impl CommandExecutor for ModifyExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let slots = SlotsArgumentType::get(context, "slots")?;
        let modifier = item_modifier(context)?;
        let params = LootContextParameters {
            world: Some(context.world()),
            position: Some(context.source.position),
            this_entity: context.source.entity.as_deref(),
            ..Default::default()
        };
        let modify = |stack: &ItemStack| apply_item_modifier(&modifier, stack, &params);
        // Only filled slots are modified, and only they count as selected.
        let mut selected = 0;

        if self.block {
            let pos = BlockPosArgumentType::get_loaded_block_pos(context, "target")?;
            let inventory = context
                .world()
                .get_block_entity(&pos)
                .and_then(BlockEntity::get_inventory)
                .ok_or_else(|| {
                    ERROR_NOT_CONTAINER.create_without_context(
                        TextComponent::text(pos.0.x.to_string()),
                        TextComponent::text(pos.0.y.to_string()),
                        TextComponent::text(pos.0.z.to_string()),
                    )
                })?;
            for &slot in slots.iter().filter(|&&slot| slot < inventory.size()) {
                let stack = inventory.get_stack(slot);
                if !stack.is_empty() {
                    selected += 1;
                    inventory.set_stack(slot, modify(&stack));
                }
            }
            if selected == 0 {
                return Err(no_such_slot(context));
            }
            context.source.send_feedback(
                TextComponent::translate_cross(
                    translation::java::COMMANDS_ITEM_BLOCK_MODIFY_SUCCESS,
                    translation::java::COMMANDS_ITEM_BLOCK_MODIFY_SUCCESS,
                    [
                        TextComponent::text(selected.to_string()),
                        TextComponent::text(pos.0.x.to_string()),
                        TextComponent::text(pos.0.y.to_string()),
                        TextComponent::text(pos.0.z.to_string()),
                    ],
                ),
                true,
            );
            return Ok(selected);
        }

        let targets = EntityArgumentType::get_entities(context, "target")?;
        let mut total = 0;
        let mut changed_targets = Vec::new();
        for target in &targets {
            let mut updated = 0;
            for &slot in slots {
                let Some(stack) = entity_slot_items(target.as_ref(), &[slot]).pop() else {
                    continue;
                };
                if stack.is_empty() {
                    continue;
                }
                selected += 1;
                if set_entity_slot(target.as_ref(), slot, &modify(&stack)) {
                    updated += 1;
                }
            }
            if updated > 0 {
                total += updated;
                changed_targets.push(target);
            }
        }
        if selected == 0 {
            return Err(no_such_slot(context));
        }
        // `CommandResponseTracker.sendFeedback` over the entities that changed.
        let message = match changed_targets.as_slice() {
            [] => return Err(ERROR_NO_CHANGES.create_without_context()),
            [target] => TextComponent::translate_cross(
                translation::java::COMMANDS_ITEM_ENTITY_MODIFY_SUCCESS_SINGLE,
                translation::java::COMMANDS_ITEM_ENTITY_MODIFY_SUCCESS_SINGLE,
                [
                    TextComponent::text(total.to_string()),
                    target.get_display_name(),
                ],
            ),
            targets => TextComponent::translate_cross(
                translation::java::COMMANDS_ITEM_ENTITY_MODIFY_SUCCESS_MULTIPLE,
                translation::java::COMMANDS_ITEM_ENTITY_MODIFY_SUCCESS_MULTIPLE,
                [TextComponent::text(targets.len().to_string())],
            ),
        };
        context.source.send_feedback(message, true);
        Ok(total)
    }
}

/// `ResourceOrIdArgument.getLootModifier`: an inline modifier, or one from the datapacks.
fn item_modifier(context: &CommandContext) -> Result<serde_json::Value, CommandSyntaxError> {
    match LootResourceArgumentType::get(context, "modifier")? {
        LootResource::Id(id) => context
            .server()
            .datapack_manager
            .get_item_modifier(&id.to_string())
            .ok_or_else(|| {
                ERROR_NO_SUCH_MODIFIER.create_without_context(
                    TextComponent::text(id.to_string()),
                    TextComponent::text("minecraft:item_modifier"),
                )
            }),
        LootResource::Inline(value) => Ok(value.as_ref().clone()),
        LootResource::InlineTable(_) => Err(ERROR_NO_CHANGES.create_without_context()),
    }
}

/// `ERROR_TARGET_INAPPLICABLE_SLOT` with the slot as it was written.
fn no_such_slot(context: &CommandContext) -> CommandSyntaxError {
    let name = context
        .arguments
        .get("slots")
        .and_then(|argument| context.input.get(argument.range.start..argument.range.end))
        .unwrap_or_default();
    ERROR_NO_SUCH_SLOT.create_without_context(TextComponent::text(name.to_string()))
}

fn modify_node(target: RequiredArgumentBuilder, block: bool) -> RequiredArgumentBuilder {
    target.then(
        argument(
            "slots",
            ClientParsed::new(SlotsArgumentType, JavaClientArgumentType::SlotSource),
        )
        .then(
            argument("modifier", LootResourceArgumentType(LootRegistry::Modifier))
                .executes(ModifyExecutor { block }),
        ),
    )
}

fn block_item_node() -> RequiredArgumentBuilder {
    argument("item", ItemStackArgumentType)
        .executes(BlockReplaceExecutor { has_count: false })
        .then(
            argument("count", IntegerArgumentType::new(1, 99))
                .executes(BlockReplaceExecutor { has_count: true }),
        )
}

fn entity_item_node() -> RequiredArgumentBuilder {
    argument("item", ItemStackArgumentType)
        .executes(EntityReplaceExecutor { has_count: false })
        .then(
            argument("count", IntegerArgumentType::new(1, 99))
                .executes(EntityReplaceExecutor { has_count: true }),
        )
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let block_node = literal("block").then(
        argument("target", BlockPosArgumentType).then(
            argument(
                "slots",
                ClientParsed::new(SlotArgumentType, JavaClientArgumentType::SlotSource),
            )
            .then(literal("with").then(block_item_node())),
        ),
    );

    let entity_node = literal("entity").then(
        argument("target", EntityArgumentType::Entities).then(
            argument(
                "slots",
                ClientParsed::new(SlotArgumentType, JavaClientArgumentType::SlotSource),
            )
            .then(literal("with").then(entity_item_node())),
        ),
    );

    dispatcher.register(
        command("item", DESCRIPTION)
            .requires(PERMISSION)
            .then(literal("replace").then(block_node).then(entity_node))
            .then(
                literal("modify")
                    .then(
                        literal("block")
                            .then(modify_node(argument("target", BlockPosArgumentType), true)),
                    )
                    .then(literal("entity").then(modify_node(
                        argument("target", EntityArgumentType::Entities),
                        false,
                    ))),
            ),
    );
}
