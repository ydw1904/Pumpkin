use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;

use super::BlockEntity;

// todo: CustomName, LastExecution
pub struct CommandBlockEntity {
    pub position: BlockPos,
    pub powered: AtomicBool,
    pub condition_met: AtomicBool,
    pub auto: AtomicBool,
    pub dirty: AtomicBool,
    pub command: StdMutex<String>,
    pub last_output: StdMutex<Option<TextComponent>>,
    pub track_output: AtomicBool,
    pub success_count: AtomicU32,
    /// Vanilla `BaseCommandBlock.updateLastExecution`: run at most once per tick.
    pub update_last_execution: AtomicBool,
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
            track_output: AtomicBool::new(track_output),
            success_count: AtomicU32::new(0),
            update_last_execution: AtomicBool::new(true),
        }
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
        nbt.put_bool(
            "UpdateLastExecution",
            self.update_last_execution.load(Ordering::SeqCst),
        );
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
        let success_count =
            AtomicU32::new(nbt.get_int("SuccessCount").unwrap_or(0).cast_unsigned());
        let update_last_execution =
            AtomicBool::new(nbt.get_bool("UpdateLastExecution").unwrap_or(true));

        Self {
            position,
            condition_met,
            auto,
            powered,
            command,
            last_output,
            track_output,
            success_count,
            update_last_execution,
            dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_sync_nbt(nbt);
    }

    // Vanilla sends a command block's data only to whoever opens it (`openCommandBlock`), never
    // in chunk data. Pumpkin opens the screen through the block entity update.
    fn update_packet_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.write_sync_nbt(&mut nbt);
        Some(nbt)
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
        Some(crate::block::entities::bedrock::build_command_block_actor(
            self.position,
            &command,
            None,
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
