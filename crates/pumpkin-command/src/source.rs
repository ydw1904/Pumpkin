use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use std::sync::Arc;

use crate::argument_types::entity_anchor::EntityAnchor;
use crate::errors::command_syntax_error::CommandSyntaxError;

pub trait ReturnValueCallable: Send + Sync {
    fn call(&self, value: ReturnValue);
}

impl<F: Fn(ReturnValue) + Send + Sync> ReturnValueCallable for F {
    fn call(&self, value: ReturnValue) {
        self(value);
    }
}

pub type ReturnValueCallback = Arc<dyn ReturnValueCallable>;

/// The callbacks a command's result goes to. Clones share the list, since every command
/// source carries one and sources are copied for each command they run.
#[derive(Clone, Default)]
pub struct ResultValueTaker(Option<Arc<[ReturnValueCallback]>>);

impl ResultValueTaker {
    #[must_use]
    pub fn merge(taker_1: &Self, taker_2: &Self) -> Self {
        match (&taker_1.0, &taker_2.0) {
            (None, _) => taker_2.clone(),
            (_, None) => taker_1.clone(),
            (Some(takers_1), Some(takers_2)) => Self(Some(
                takers_1.iter().chain(takers_2.iter()).cloned().collect(),
            )),
        }
    }

    #[must_use]
    pub const fn new() -> Self {
        Self(None)
    }

    /// A taker with the one callback.
    #[must_use]
    pub fn of(callback: ReturnValueCallback) -> Self {
        Self(Some(Arc::new([callback])))
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn call(&self, return_value: ReturnValue) {
        for callback in self.0.iter().flat_map(|takers| takers.iter()) {
            callback.call(return_value);
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ReturnValue {
    Success(i32),
    Failure,
}

impl ReturnValue {
    #[must_use]
    pub const fn success_value(self) -> bool {
        match self {
            Self::Success(_) => true,
            Self::Failure => false,
        }
    }

    #[must_use]
    pub const fn result_value(self) -> i32 {
        match self {
            Self::Success(value) => value,
            Self::Failure => 0,
        }
    }
}

/// Abstract command source trait for command dispatch and execution.
pub trait CommandSource: Clone + Send + Sync + 'static {
    /// Sends a feedback message to the command sender.
    fn send_message(&self, message: TextComponent);

    /// Sends an error message to the command sender.
    fn send_error(&self, error: TextComponent) {
        self.send_message(error);
    }

    /// Invokes callbacks for the command's return value.
    fn call_result(&self, _result: ReturnValue) {}

    /// Checks if this command source has the specified permission.
    fn has_permission(&self, _permission: &str) -> bool {
        true
    }

    /// Returns the position of this command source in the world.
    fn position(&self) -> Vector3<f64> {
        Vector3::default()
    }

    /// Returns the rotation (pitch, yaw) of this command source.
    fn rotation(&self) -> Vector2<f32> {
        Vector2::default()
    }

    /// Checks if the block at the given position is loaded.
    fn check_block_loaded(&self, _pos: &BlockPos) -> Result<(), CommandSyntaxError> {
        Ok(())
    }

    /// Returns the anchor position for the given entity anchor.
    fn anchor_position(&self, _anchor: EntityAnchor) -> Vector3<f64> {
        self.position()
    }

    /// Returns the current entity anchor of this command source.
    fn entity_anchor(&self) -> EntityAnchor {
        EntityAnchor::Feet
    }

    /// Whether this source runs under `return run` (vanilla `ChainModifiers.isReturn`).
    fn is_returning(&self) -> bool {
        false
    }

    /// This source with its frame's return value consumer chained to its callbacks, for the
    /// command a `return run` chain ends in.
    #[must_use]
    fn with_frame_return_callback(&self) -> Self {
        self.clone()
    }

    /// Vanilla `FallthroughTask`: a `return run` chain that ran nothing returns failure.
    fn fall_through(&self) {}

    /// The most sources a chain may fork into (vanilla `ExecutionContext.forkLimit`).
    fn fork_limit(&self) -> usize {
        usize::MAX
    }

    /// Runs a parsed command in this source's command queue, creating one for a top-level
    /// command (vanilla `Commands.executeCommandInContext`).
    fn execute_in_context(&self, run: &mut dyn FnMut()) {
        run();
    }

    /// Counts one unit against the command queue's limit (vanilla
    /// `ExecutionContext.incrementCost`).
    fn increment_command_cost(&self) {}

    /// Whether the command queue has used up its limit, so no further command runs.
    fn command_quota_exhausted(&self) -> bool {
        false
    }

    /// The block ids of a block tag such as `minecraft:logs`, including the tags datapacks
    /// define, or `None` for an unknown tag.
    fn block_tag(&self, tag: &str) -> Option<Vec<u16>> {
        crate::argument_types::block_predicate::builtin_block_tag(tag)
    }
}

#[derive(Clone, Default)]
pub struct DummySource {
    pub position: Vector3<f64>,
    pub rotation: Vector2<f32>,
    pub entity_anchor: EntityAnchor,
    pub command_result_taker: ResultValueTaker,
}

impl DummySource {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn dummy() -> Self {
        Self::default()
    }
}

impl CommandSource for DummySource {
    fn send_message(&self, _message: TextComponent) {}
    fn send_error(&self, _error: TextComponent) {}
    fn position(&self) -> Vector3<f64> {
        self.position
    }
    fn rotation(&self) -> Vector2<f32> {
        self.rotation
    }
    fn entity_anchor(&self) -> EntityAnchor {
        self.entity_anchor
    }
    fn call_result(&self, result: ReturnValue) {
        self.command_result_taker.call(result);
    }
}

impl CommandSource for () {
    fn send_message(&self, _message: TextComponent) {}
}

impl<S: CommandSource> CommandSource for Arc<S> {
    fn send_message(&self, message: TextComponent) {
        (**self).send_message(message);
    }

    fn send_error(&self, error: TextComponent) {
        (**self).send_error(error);
    }

    fn has_permission(&self, permission: &str) -> bool {
        (**self).has_permission(permission)
    }

    fn position(&self) -> Vector3<f64> {
        (**self).position()
    }

    fn rotation(&self) -> Vector2<f32> {
        (**self).rotation()
    }

    fn entity_anchor(&self) -> EntityAnchor {
        (**self).entity_anchor()
    }

    fn anchor_position(&self, anchor: EntityAnchor) -> Vector3<f64> {
        (**self).anchor_position(anchor)
    }

    fn call_result(&self, result: ReturnValue) {
        (**self).call_result(result);
    }

    fn check_block_loaded(&self, pos: &BlockPos) -> Result<(), CommandSyntaxError> {
        (**self).check_block_loaded(pos)
    }

    fn is_returning(&self) -> bool {
        (**self).is_returning()
    }

    fn with_frame_return_callback(&self) -> Self {
        Self::new((**self).with_frame_return_callback())
    }

    fn fall_through(&self) {
        (**self).fall_through();
    }

    fn fork_limit(&self) -> usize {
        (**self).fork_limit()
    }

    fn execute_in_context(&self, run: &mut dyn FnMut()) {
        (**self).execute_in_context(run);
    }

    fn increment_command_cost(&self) {
        (**self).increment_command_cost();
    }

    fn command_quota_exhausted(&self) -> bool {
        (**self).command_quota_exhausted()
    }
}
