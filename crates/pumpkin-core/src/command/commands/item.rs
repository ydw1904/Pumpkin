use std::sync::Arc;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::translation;
use pumpkin_inventory::Inventory;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
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
use crate::command::argument_types::slot::SlotsArgumentType;
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

/// `ItemStack.getDisplayName` as the item commands show it.
fn item_display_name(stack: &ItemStack) -> TextComponent {
    TextComponent::text("[")
        .add_child(stack.item.translated_name())
        .add_child(TextComponent::text("]"))
        .hover_event(HoverEvent::ShowItem {
            id: stack.item.registry_key.to_string().into(),
            count: Some(i32::from(stack.item_count)),
            components: None,
        })
}

/// Vanilla `ItemAccessor`: the container of a block, or the entities of a selector.
enum ItemAccessor {
    Block(BlockPos),
    Entities(Vec<Arc<dyn EntityBase>>),
}

impl ItemAccessor {
    /// `ArgProvider.access`: the loaded block position or the selected entities.
    fn access(
        context: &CommandContext,
        name: &str,
        block: bool,
    ) -> Result<Self, CommandSyntaxError> {
        Ok(if block {
            Self::Block(BlockPosArgumentType::get_loaded_block_pos(context, name)?)
        } else {
            Self::Entities(EntityArgumentType::get_entities(context, name)?)
        })
    }

    /// `BlockItemAccessor.getContainer`.
    fn container(
        context: &CommandContext,
        pos: &BlockPos,
        error: &'static CommandErrorType<3>,
    ) -> Result<Arc<dyn Inventory>, CommandSyntaxError> {
        context
            .world()
            .get_block_entity(pos)
            .and_then(BlockEntity::get_container)
            .ok_or_else(|| {
                error.create_without_context(
                    TextComponent::text(pos.0.x.to_string()),
                    TextComponent::text(pos.0.y.to_string()),
                    TextComponent::text(pos.0.z.to_string()),
                )
            })
    }

    /// `ItemAccessor.getSlots(...).itemCopies()`: every slot of the range the source has, empty
    /// ones included.
    fn item_copies(
        &self,
        context: &CommandContext,
        slots: &[usize],
    ) -> Result<Vec<ItemStack>, CommandSyntaxError> {
        Ok(match self {
            Self::Block(pos) => {
                let inventory = Self::container(context, pos, &ERROR_SOURCE_NOT_A_CONTAINER)?;
                slots
                    .iter()
                    .filter(|&&slot| slot < inventory.size())
                    .map(|&slot| inventory.get_stack(slot))
                    .collect()
            }
            Self::Entities(entities) => entities
                .iter()
                .flat_map(|entity| entity_slot_items(entity.as_ref(), slots))
                .collect(),
        })
    }
}

/// Vanilla `ItemProvider.of`: hands out the items once, in order.
struct ItemProvider<'a> {
    items: &'a [ItemStack],
    next: usize,
}

impl ItemProvider<'_> {
    const fn restart(&mut self) {
        self.next = 0;
    }

    const fn has_next(&self) -> bool {
        self.next < self.items.len()
    }

    fn next(&mut self) -> ItemStack {
        let stack = self.items[self.next].clone();
        self.next += 1;
        stack
    }
}

/// Vanilla `ItemCommands.setItems`: fills the target slots from `items`, one target at a time.
fn set_items(
    context: &CommandContext,
    target: &ItemAccessor,
    items: &[ItemStack],
    known_item: Option<&ItemStack>,
) -> CommandExecutorResult {
    let slots = SlotsArgumentType::get(context, "slots")?;
    let mut items = ItemProvider { items, next: 0 };
    // `SlotSelector.tracking`: slots offered an item.
    let mut selected = 0;
    let known_name = known_item.map(item_display_name);
    match target {
        ItemAccessor::Block(pos) => {
            let inventory = ItemAccessor::container(context, pos, &ERROR_NOT_CONTAINER)?;
            let mut count = 0;
            for &slot in slots.iter().filter(|&&slot| slot < inventory.size()) {
                if !items.has_next() {
                    break;
                }
                selected += 1;
                inventory.set_stack(slot, items.next());
                count += 1;
            }
            if selected == 0 {
                return Err(no_such_slot(context, "slots"));
            }
            let (x, y, z) = (
                TextComponent::text(pos.0.x.to_string()),
                TextComponent::text(pos.0.y.to_string()),
                TextComponent::text(pos.0.z.to_string()),
            );
            let message = match known_name {
                Some(name) => TextComponent::translate_cross(
                    translation::java::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS_KNOWN_ITEM,
                    translation::java::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS_KNOWN_ITEM,
                    [TextComponent::text(count.to_string()), x, y, z, name],
                ),
                None => TextComponent::translate_cross(
                    translation::java::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS,
                    translation::java::COMMANDS_ITEM_BLOCK_REPLACE_SUCCESS,
                    [TextComponent::text(count.to_string()), x, y, z],
                ),
            };
            context.source.send_feedback(message, true);
            Ok(count)
        }
        ItemAccessor::Entities(entities) => {
            let mut total = 0;
            // `CommandResponseTracker` over the entities with a changed slot.
            let mut changed = Vec::new();
            for entity in entities {
                items.restart();
                let mut count = 0;
                for &slot in slots {
                    if !items.has_next() {
                        break;
                    }
                    if entity_slot_items(entity.as_ref(), &[slot]).is_empty() {
                        continue;
                    }
                    selected += 1;
                    if set_entity_slot(entity.as_ref(), slot, &items.next()) {
                        count += 1;
                    }
                }
                if count > 0 {
                    total += count;
                    changed.push(entity);
                }
            }
            if selected == 0 {
                return Err(no_such_slot(context, "slots"));
            }
            let message = entity_replace_message(&changed, total, known_name)?;
            context.source.send_feedback(message, true);
            Ok(total)
        }
    }
}

/// `EntityItemAccessor.RESPONSE_SET` and `RESPONSE_SET_KNOWN_ITEM` over the changed entities.
fn entity_replace_message(
    changed: &[&Arc<dyn EntityBase>],
    total: i32,
    known_name: Option<TextComponent>,
) -> Result<TextComponent, CommandSyntaxError> {
    Ok(match (changed, known_name) {
        ([], Some(name)) => return Err(ERROR_NO_CHANGES_KNOWN_ITEM.create_without_context(name)),
        ([], None) => return Err(ERROR_NO_CHANGES.create_without_context()),
        ([entity], Some(name)) => TextComponent::translate_cross(
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE_KNOWN_ITEM,
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE_KNOWN_ITEM,
            [
                TextComponent::text(total.to_string()),
                entity.get_display_name(),
                name,
            ],
        ),
        ([entity], None) => TextComponent::translate_cross(
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE,
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_SINGLE,
            [
                TextComponent::text(total.to_string()),
                entity.get_display_name(),
            ],
        ),
        (changed, Some(name)) => TextComponent::translate_cross(
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE_KNOWN_ITEM,
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE_KNOWN_ITEM,
            [TextComponent::text(changed.len().to_string()), name],
        ),
        (changed, None) => TextComponent::translate_cross(
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE,
            translation::java::COMMANDS_ITEM_ENTITY_REPLACE_SUCCESS_MULTIPLE,
            [TextComponent::text(changed.len().to_string())],
        ),
    })
}

/// `/item replace <target> <slots> with <item> [<count>]`.
struct WithExecutor {
    block: bool,
    has_count: bool,
}

impl CommandExecutor for WithExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let target = ItemAccessor::access(context, "target", self.block)?;
        let mut stack = ItemStackArgumentType::get(context, "item")?;
        let count = if self.has_count {
            IntegerArgumentType::get(context, "count")?
        } else {
            1
        };
        check_stack_size(&stack, count)?;
        stack.item_count = count as u8;
        set_items(context, &target, std::slice::from_ref(&stack), Some(&stack))
    }
}

/// `/item replace <target> <slots> from <source> <sourceSlots> [<modifier>]`.
struct FromExecutor {
    block: bool,
    source_block: bool,
    has_modifier: bool,
}

impl CommandExecutor for FromExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let target = ItemAccessor::access(context, "target", self.block)?;
        // Vanilla `ItemCommands.getItems`.
        let source = ItemAccessor::access(context, "source", self.source_block)?;
        let source_slots = SlotsArgumentType::get(context, "sourceSlots")?;
        let modifier = self
            .has_modifier
            .then(|| item_modifier(context))
            .transpose()?;
        let mut items = source.item_copies(context, source_slots)?;
        if let Some(modifier) = &modifier {
            for stack in &mut items {
                *stack = apply_modifier(context, modifier, stack);
            }
        }
        if items.is_empty() {
            return Err(
                ERROR_SOURCE_NO_SUCH_SLOT.create_without_context(TextComponent::text(
                    argument_text(context, "sourceSlots"),
                )),
            );
        }
        set_items(context, &target, &items, None)
    }
}

/// Vanilla `ItemCommands.applyModifier`: the modified stack, cut down to its stack size.
fn apply_modifier(
    context: &CommandContext,
    modifier: &serde_json::Value,
    stack: &ItemStack,
) -> ItemStack {
    let params = LootContextParameters {
        world: Some(context.world()),
        position: Some(context.source.position),
        this_entity: context.source.entity.as_deref(),
        ..Default::default()
    };
    let mut stack = apply_item_modifier(modifier, stack, &params);
    stack.item_count = stack.item_count.min(stack.get_max_stack_size());
    stack
}

const ERROR_SOURCE_NOT_A_CONTAINER: CommandErrorType<3> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_SOURCE_NOT_A_CONTAINER,
    translation::java::COMMANDS_ITEM_SOURCE_NOT_A_CONTAINER,
);

const ERROR_SOURCE_NO_SUCH_SLOT: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_SOURCE_NO_SUCH_SLOT,
    translation::java::COMMANDS_ITEM_SOURCE_NO_SUCH_SLOT,
);

const ERROR_NO_CHANGES_KNOWN_ITEM: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_ITEM_TARGET_FAILED_KNOWN_ITEM,
    translation::java::COMMANDS_ITEM_TARGET_FAILED_KNOWN_ITEM,
);

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
        let modify = |stack: &ItemStack| apply_modifier(context, &modifier, stack);
        // Only filled slots are modified, and only they count as selected.
        let mut selected = 0;

        if self.block {
            let pos = BlockPosArgumentType::get_loaded_block_pos(context, "target")?;
            let inventory = context
                .world()
                .get_block_entity(&pos)
                .and_then(BlockEntity::get_container)
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
                return Err(no_such_slot(context, "slots"));
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
            return Err(no_such_slot(context, "slots"));
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

/// The argument `name` as it was written.
fn argument_text(context: &CommandContext, name: &str) -> String {
    context
        .arguments
        .get(name)
        .and_then(|argument| context.input.get(argument.range.start..argument.range.end))
        .unwrap_or_default()
        .to_string()
}

/// `ERROR_TARGET_INAPPLICABLE_SLOT` with the slot as it was written.
fn no_such_slot(context: &CommandContext, name: &str) -> CommandSyntaxError {
    ERROR_NO_SUCH_SLOT.create_without_context(TextComponent::text(argument_text(context, name)))
}

/// Vanilla `BlockItemAccessor.PROVIDER` or `EntityItemAccessor.PROVIDER`.
fn accessor_argument(name: &'static str, block: bool) -> RequiredArgumentBuilder {
    if block {
        argument(name, BlockPosArgumentType)
    } else {
        argument(name, EntityArgumentType::Entities)
    }
}

fn slots_argument(name: &'static str) -> RequiredArgumentBuilder {
    argument(
        name,
        ClientParsed::new(SlotsArgumentType, JavaClientArgumentType::SlotSource),
    )
}

fn modifier_argument() -> RequiredArgumentBuilder {
    argument("modifier", LootResourceArgumentType(LootRegistry::Modifier))
}

fn modify_node(block: bool) -> RequiredArgumentBuilder {
    accessor_argument("target", block)
        .then(slots_argument("slots").then(modifier_argument().executes(ModifyExecutor { block })))
}

fn from_node(block: bool, source_block: bool) -> RequiredArgumentBuilder {
    let from = |has_modifier| FromExecutor {
        block,
        source_block,
        has_modifier,
    };
    accessor_argument("source", source_block).then(
        slots_argument("sourceSlots")
            .executes(from(false))
            .then(modifier_argument().executes(from(true))),
    )
}

/// Vanilla `ItemCommands.wrapSetItems`.
fn set_items_node(block: bool) -> RequiredArgumentBuilder {
    accessor_argument("target", block).then(
        slots_argument("slots")
            .then(
                literal("from")
                    .then(literal("entity").then(from_node(block, false)))
                    .then(literal("block").then(from_node(block, true))),
            )
            .then(
                literal("with").then(
                    argument("item", ItemStackArgumentType)
                        .executes(WithExecutor {
                            block,
                            has_count: false,
                        })
                        .then(argument("count", IntegerArgumentType::new(1, 99)).executes(
                            WithExecutor {
                                block,
                                has_count: true,
                            },
                        )),
                ),
            ),
    )
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("item", DESCRIPTION)
            .requires(PERMISSION)
            .then(
                literal("replace")
                    .then(literal("entity").then(set_items_node(false)))
                    .then(literal("block").then(set_items_node(true))),
            )
            .then(
                literal("modify")
                    .then(literal("entity").then(modify_node(false)))
                    .then(literal("block").then(modify_node(true))),
            ),
    );
}
