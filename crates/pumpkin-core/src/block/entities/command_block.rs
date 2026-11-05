use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering},
};

use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;

use super::BlockEntity;
use crate::world::World;

/// Vanilla `BaseCommandBlock.NO_LAST_EXECUTION`.
pub const NO_LAST_EXECUTION: i64 = -1;
/// Vanilla `BaseCommandBlock.DEFAULT_NAME`.
const DEFAULT_NAME: &str = "@";

pub struct CommandBlockEntity {
    pub position: BlockPos,
    pub powered: AtomicBool,
    pub condition_met: AtomicBool,
    pub auto: AtomicBool,
    pub dirty: AtomicBool,
    pub command: StdMutex<String>,
    pub last_output: StdMutex<Option<TextComponent>>,
    /// Vanilla `BaseCommandBlock.customName`, the command source's name when set.
    pub custom_name: StdMutex<Option<TextComponent>>,
    pub track_output: AtomicBool,
    pub success_count: AtomicU32,
    /// Vanilla `BaseCommandBlock.updateLastExecution`: run at most once per tick.
    pub update_last_execution: AtomicBool,
    /// Vanilla `BaseCommandBlock.lastExecution`: the game time of the last run.
    pub last_execution: AtomicI64,
}

impl CommandBlockEntity {
    pub const ID: &'static str = "minecraft:command_block";
    #[must_use]
    pub const fn new(position: BlockPos, track_output: bool, is_chain: bool) -> Self {
        Self {
            position,
            powered: AtomicBool::new(false),
            condition_met: AtomicBool::new(false),
            auto: AtomicBool::new(is_chain),
            dirty: AtomicBool::new(false),
            command: StdMutex::new(String::new()),
            last_output: StdMutex::new(None),
            custom_name: StdMutex::new(None),
            track_output: AtomicBool::new(track_output),
            success_count: AtomicU32::new(0),
            update_last_execution: AtomicBool::new(true),
            last_execution: AtomicI64::new(NO_LAST_EXECUTION),
        }
    }

    /// Vanilla `BaseCommandBlock.getName`.
    #[must_use]
    pub fn name(&self) -> TextComponent {
        self.custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| TextComponent::text(DEFAULT_NAME))
    }

    fn write_sync_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("auto", self.auto.load(Ordering::SeqCst));
        nbt.put_string(
            "Command",
            self.command
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .to_string(),
        );
        nbt.put_bool("conditionMet", self.condition_met.load(Ordering::SeqCst));
        if let Some(custom_name) = self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            nbt.put("CustomName", custom_name.0.to_nbt_tag());
        }
        let track_output = self.track_output.load(Ordering::SeqCst);
        // Vanilla `BaseCommandBlock.save` stores the output as a component, and only when tracked.
        if track_output
            && let Some(last_output) = self
                .last_output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
        {
            nbt.put("LastOutput", last_output.0.to_nbt_tag());
        }
        nbt.put_bool("powered", self.powered.load(Ordering::SeqCst));
        nbt.put_bool("TrackOutput", track_output);
        let update_last_execution = self.update_last_execution.load(Ordering::SeqCst);
        nbt.put_bool("UpdateLastExecution", update_last_execution);
        let last_execution = self.last_execution.load(Ordering::SeqCst);
        if update_last_execution && last_execution != NO_LAST_EXECUTION {
            nbt.put_long("LastExecution", last_execution);
        }
        nbt.put_int(
            "SuccessCount",
            self.success_count.load(Ordering::SeqCst).cast_signed(),
        );
    }
}

impl BlockEntity for CommandBlockEntity {
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
        let condition_met = AtomicBool::new(nbt.get_bool("conditionMet").unwrap_or(false));
        let auto = AtomicBool::new(nbt.get_bool("auto").unwrap_or(false));
        let powered = AtomicBool::new(nbt.get_bool("powered").unwrap_or(false));
        let command = StdMutex::new(nbt.get_string("Command").unwrap_or("").to_string());
        let tracks_output = nbt.get_bool("TrackOutput").unwrap_or(true);
        let last_output = StdMutex::new(
            nbt.get("LastOutput")
                .filter(|_| tracks_output)
                .map(TextComponent::from_nbt),
        );
        let track_output = AtomicBool::new(tracks_output);
        let custom_name = StdMutex::new(nbt.get("CustomName").map(TextComponent::from_nbt));
        let success_count =
            AtomicU32::new(nbt.get_int("SuccessCount").unwrap_or(0).cast_unsigned());
        let updates_last_execution = nbt.get_bool("UpdateLastExecution").unwrap_or(true);
        let last_execution = AtomicI64::new(if updates_last_execution {
            nbt.get_long("LastExecution").unwrap_or(NO_LAST_EXECUTION)
        } else {
            NO_LAST_EXECUTION
        });
        let update_last_execution = AtomicBool::new(updates_last_execution);

        Self {
            position,
            condition_met,
            auto,
            powered,
            command,
            last_output,
            custom_name,
            track_output,
            success_count,
            update_last_execution,
            last_execution,
            dirty: AtomicBool::new(false),
        }
    }

    /// The `setAutomatic` call in vanilla `CommandBlockEntity.loadAdditional`: turning `auto`
    /// on schedules the first tick of an unpowered impulse or repeating block.
    fn loaded_in_world(&self, world: &Arc<World>) {
        let was_auto = world
            .get_block_entity(&self.position)
            .is_some_and(|previous| {
                previous
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|previous| previous.auto.load(Ordering::Relaxed))
            });
        if !was_auto
            && self.auto.load(Ordering::Relaxed)
            && !self.powered.load(Ordering::Relaxed)
            && world.get_block(&self.position) != &pumpkin_data::Block::CHAIN_COMMAND_BLOCK
        {
            crate::block::blocks::command::CommandBlock::schedule_automatic_tick(
                world,
                self,
                &self.position,
            );
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_sync_nbt(nbt);
    }

    fn collect_item_components(&self, stack: &mut ItemStack) {
        if let Some(name) = self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            stack.set_data_component(CustomNameImpl { name });
        }
    }

    /// Vanilla `applyImplicitComponents`: the item's name, or none, replaces the custom name.
    fn apply_item_components(&self, stack: &ItemStack) {
        *self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stack
            .get_data_component::<CustomNameImpl>()
            .map(|custom_name| custom_name.name.clone());
    }

    fn bedrock_block_actor_data(
        &self,
        state_id: pumpkin_data::BlockStateId,
    ) -> Option<NbtCompound> {
        let conditional =
            pumpkin_data::block_properties::CommandBlockProperties::from_state_id(state_id)
                .conditional;
        let command = self
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let last_output = self
            .last_output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|output| output.clone().get_text())
            .unwrap_or_default();
        let custom_name = self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|name| name.clone().get_text());
        Some(build_command_block_actor(
            self.position,
            &command,
            custom_name.as_deref(),
            self.auto.load(Ordering::Relaxed),
            self.condition_met.load(Ordering::Relaxed),
            conditional,
            self.powered.load(Ordering::Relaxed),
            self.success_count.load(Ordering::Relaxed),
            self.track_output.load(Ordering::Relaxed),
            &last_output,
        ))
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Builds the Bedrock block-actor NBT for command blocks.
#[expect(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[must_use]
pub fn build_command_block_actor(
    position: BlockPos,
    command: &str,
    custom_name: Option<&str>,
    auto: bool,
    condition_met: bool,
    conditional: bool,
    powered: bool,
    success_count: u32,
    track_output: bool,
    last_output: &str,
) -> NbtCompound {
    super::bedrock::BedrockBlockActorBuilder::new("CommandBlock", position)
        .movable(false)
        .custom_name(custom_name)
        .put_string("Command", command)
        .put_bool("auto", auto)
        .put_bool("conditionMet", condition_met)
        .put_bool("conditionalMode", conditional)
        .put_bool("powered", powered)
        .put_int("SuccessCount", success_count as i32)
        .put_bool("TrackOutput", track_output)
        .put_string("LastOutput", last_output)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_block_actor_builder() {
        let pos = BlockPos::new(6, 64, 6);
        let nbt = build_command_block_actor(
            pos,
            "say hi",
            Some("Cmd"),
            true,
            true,
            false,
            false,
            1,
            true,
            "hi",
        );
        assert_eq!(nbt.get_string("id"), Some("CommandBlock"));
        assert_eq!(nbt.get_string("Command"), Some("say hi"));
        assert_eq!(nbt.get_string("CustomName"), Some("Cmd"));
        assert_eq!(nbt.get_bool("auto"), Some(true));
        assert_eq!(nbt.get_bool("conditionMet"), Some(true));
        assert_eq!(nbt.get_bool("conditionalMode"), Some(false));
        assert_eq!(nbt.get_bool("powered"), Some(false));
        assert_eq!(nbt.get_int("SuccessCount"), Some(1));
        assert_eq!(nbt.get_bool("isMovable"), Some(false));
    }
}
