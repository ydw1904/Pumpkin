use crate::command::argument_types::entity_anchor::{EntityAnchor, EntityAnchorExt};
use crate::command::execution::ExecutionContext;
use crate::command::{CommandSender, level_has_permission};
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::server::Server;
use crate::world::World;
use pumpkin_command::errors::command_syntax_error::CommandSyntaxError;
use pumpkin_command::errors::error_types::CommandErrorType;
pub use pumpkin_command::source::{
    ResultValueTaker, ReturnValue, ReturnValueCallable, ReturnValueCallback,
};
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::{Color, NamedColor};
use std::sync::Arc;

pub const REQUIRES_PLAYER: CommandErrorType<0> = CommandErrorType::new(
    translation::java::PERMISSIONS_REQUIRES_PLAYER,
    translation::java::PERMISSIONS_REQUIRES_PLAYER,
);
pub const REQUIRES_ENTITY: CommandErrorType<0> = CommandErrorType::new(
    translation::java::PERMISSIONS_REQUIRES_ENTITY,
    translation::java::PERMISSIONS_REQUIRES_ENTITY,
);

/// Represents a source of a command, which
/// contains its own state, which could keep track of its:
/// - position
/// - rotation
/// - world
/// - permissions
/// - name
/// - display name
/// - the internal server
/// - whether it is silent or not
/// - entity which it could represent
///
/// Not to be confused with [`CommandSender`], [`CommandSource`]
/// can be modified by commands to change how another
/// command works later in the command chain.
///
/// A source having a player and a particular position does
/// not necessarily mean that the player does have that position;
/// but rather is a state for more complex functionality.
#[derive(Clone)]
pub struct CommandSource {
    pub output: CommandSender,
    pub world: Option<Arc<World>>,
    pub entity: Option<Arc<dyn EntityBase>>,
    pub position: Vector3<f64>,
    pub rotation: Vector2<f32>,
    pub name: String,
    pub display_name: TextComponent,
    pub server: Option<Arc<Server>>,
    pub silent: bool,
    pub command_result_taker: ResultValueTaker,
    pub entity_anchor: EntityAnchor,
    pub frame: Frame,
    /// Vanilla `ChainModifiers.isReturn`: this source runs the rest of a `return run` chain.
    pub returning: bool,
    /// A fixed permission level that replaces the output's permissions: a function's commands
    /// get `function-permission-level`, which vanilla compiles them with.
    pub permission_level: Option<PermissionLvl>,
}

/// Vanilla `Frame`: where a `return` in a function, or in a top-level command, reports to.
#[derive(Clone, Default)]
pub struct Frame {
    /// Vanilla `Frame.depth`: how many function calls deep the commands of this frame run.
    pub depth: usize,
    /// Vanilla `Frame.returnValueConsumer`.
    pub return_value: ResultValueTaker,
    /// Vanilla `Frame.frameControl`, shared with the caller's frame under `return run`.
    pub control: FrameControl,
}

/// Vanilla `Frame.FrameControl`: what discarding a frame removes from the command queue.
#[derive(Clone, Copy, Default)]
pub enum FrameControl {
    /// A top-level command's frame: the whole queue.
    #[default]
    ClearQueue,
    /// Everything queued at this depth or deeper.
    DiscardAtDepth(usize),
}

impl Frame {
    /// Vanilla `Frame.returnSuccess`.
    pub fn return_success(&self, value: i32) {
        self.return_value.call(ReturnValue::Success(value));
    }

    /// Vanilla `Frame.returnFailure`.
    pub fn return_failure(&self) {
        self.return_value.call(ReturnValue::Failure);
    }

    /// Vanilla `Frame.discard`: nothing more runs in this frame.
    pub fn discard(&self) {
        ExecutionContext::discard(self.control);
    }
}

impl CommandSource {
    /// Creates a dummy [`CommandSource`], for unit tests and as the source stored function
    /// commands keep until they run with their own.
    ///
    /// The returned [`CommandSource`] does not contain
    /// a server or a world. If there is attempt to fetch the server or a world from
    /// the returned source, there will be a panic!
    #[must_use]
    pub(crate) fn dummy() -> Self {
        Self {
            output: CommandSender::Dummy,
            world: None,
            entity: None,
            position: Vector3::default(),
            rotation: Vector2::default(),
            name: String::new(),
            display_name: TextComponent::empty(),
            server: None,
            silent: false,
            command_result_taker: ResultValueTaker::new(),
            entity_anchor: EntityAnchor::Feet,
            frame: Frame::default(),
            returning: false,
            permission_level: None,
        }
    }

    /// Creates a usable [`CommandSource`] for running commands in an actual environment.
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        output: CommandSender,
        world: Arc<World>,
        entity: Option<Arc<dyn EntityBase>>,
        position: Vector3<f64>,
        rotation: Vector2<f32>,
        name: String,
        display_name: TextComponent,
        server: Arc<Server>,
    ) -> Self {
        Self {
            output,
            world: Some(world),
            entity,
            position,
            rotation,
            name,
            display_name,
            server: Some(server),
            silent: false,
            command_result_taker: ResultValueTaker(Vec::new()),
            entity_anchor: EntityAnchor::Feet,
            frame: Frame::default(),
            returning: false,
            permission_level: None,
        }
    }

    /// Vanilla `createCommandSourceStack` of signs and lecterns: a source with no output at the
    /// centre of `pos`, acting as `player`, or else named `name`.
    #[must_use]
    pub fn for_block(
        world: Arc<World>,
        server: Arc<Server>,
        pos: &BlockPos,
        name: &str,
        player: Option<Arc<Player>>,
    ) -> Self {
        let (name, display_name) = player.as_ref().map_or_else(
            || (name.to_string(), TextComponent::text(name.to_string())),
            |player| (player.gameprofile.name.clone(), player.get_display_name()),
        );
        // `CommandSender::Dummy` has the gamemaster level.
        Self::new(
            CommandSender::Dummy,
            world,
            player.map(|player| player as Arc<dyn EntityBase>),
            pos.to_centered_f64(),
            Vector2::new(0.0, 0.0),
            name,
            display_name,
            server,
        )
    }

    /// Returns a new [`CommandSource`] with the specified output and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_output(self, output: CommandSender) -> Self {
        Self { output, ..self }
    }

    /// Returns a new [`CommandSource`] with the specified world and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_world(self, world: Arc<World>) -> Self {
        Self {
            world: Some(world),
            silent: true,
            ..self
        }
    }

    /// Returns a new [`CommandSource`] with the specified entity and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_entity(self, entity: Arc<dyn EntityBase>) -> Self {
        let name = entity.get_name().get_text();
        let display_name = entity.get_display_name();
        Self {
            entity: Some(entity),
            name,
            display_name,
            ..self
        }
    }

    /// Returns a new [`CommandSource`] with the specified position and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_position(self, position: Vector3<f64>) -> Self {
        Self { position, ..self }
    }

    /// Returns a new [`CommandSource`] with the specified rotation and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_rotation(self, rotation: Vector2<f32>) -> Self {
        Self { rotation, ..self }
    }

    /// Merges the given takers with this one, returning a new [`CommandSource`] with
    /// the merged taker.
    #[must_use]
    pub fn merge_command_result_taker(self, command_result_taker: &ResultValueTaker) -> Self {
        let merged = ResultValueTaker::merge(&self.command_result_taker, command_result_taker);
        self.with_command_result_taker(merged)
    }

    /// Returns a new [`CommandSource`] with the specified silent state and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_silent(self) -> Self {
        Self {
            silent: true,
            ..self
        }
    }

    /// Returns a new [`CommandSource`] with the specified command result taker and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_command_result_taker(self, command_result_taker: ResultValueTaker) -> Self {
        Self {
            command_result_taker,
            ..self
        }
    }

    /// Returns a new [`CommandSource`] with the specified entity anchor and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_entity_anchor(self, entity_anchor: EntityAnchor) -> Self {
        Self {
            silent: true,
            entity_anchor,
            ..self
        }
    }

    /// Returns a new [`CommandSource`] with the rotation changed in such
    /// a way that the source faces the anchor of the entity and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_looking_at_entity(
        self,
        entity: &Arc<dyn EntityBase>,
        anchor: EntityAnchor,
    ) -> Self {
        self.with_looking_at_pos(anchor.position_at_entity(entity.get_entity()))
    }

    /// Returns a new [`CommandSource`] with the rotation changed in such
    /// a way that the source faces the provided position and
    /// everything else from the `source` provided.
    #[must_use]
    pub fn with_looking_at_pos(self, pos: Vector3<f64>) -> Self {
        let source_pos = self.entity_anchor.position_at_source(&self);
        let delta = pos.sub(&source_pos);
        let horizontal_len = delta.horizontal_length();
        let pitch = -delta.y.atan2(horizontal_len).to_degrees();
        let yaw = delta.z.atan2(delta.x).to_degrees() - 90.0;
        self.with_rotation(Vector2::new(
            wrap_degrees(pitch as f32),
            wrap_degrees(yaw as f32),
        ))
    }

    /// Gets the entity as a result:
    ///
    /// - If this source actually contains an entity, it returns that wrapped in an [`Ok`].
    /// - If it doesn't, a command error is provided instead, wrapped in an [`Err`].
    pub fn entity_or_err(&self) -> Result<Arc<dyn EntityBase>, CommandSyntaxError> {
        self.entity
            .clone()
            .ok_or(REQUIRES_ENTITY.create_without_context())
    }

    /// Gets the world as a result:
    ///
    /// - If this source actually contains a server, it returns that.
    /// - If it doesn't, this function **panics**. Ideally, a source should contain a world, but it may not in a unit test.
    #[must_use]
    pub const fn world(&self) -> &Arc<World> {
        self.world.as_ref().expect("Expected world to exist")
    }

    /// Gets the server as a result:
    ///
    /// - If this source actually contains a server, it returns that.
    /// - If it doesn't, this function **panics**. Ideally, a source should contain the server, but it may not in a unit test.
    #[must_use]
    pub const fn server(&self) -> &Arc<Server> {
        self.server.as_ref().expect("Expected server to exist")
    }

    /// Gets the player as an option:
    ///
    /// - If this source actually contains a player, it returns that wrapped in a [`Some`].
    /// - If it doesn't, a [`None`] is returned instead.
    #[must_use]
    pub fn player_or_none(&self) -> Option<&Player> {
        self.entity.as_ref().and_then(|entity| entity.get_player())
    }

    /// Gets the player as an `Arc<Player>` from the underlying output sender.
    #[must_use]
    pub fn as_player(&self) -> Option<Arc<Player>> {
        self.output.as_player()
    }

    /// Gets the player as a result:
    ///
    /// - If this source actually contains a player, it returns that wrapped in an [`Ok`].
    /// - If it doesn't, a command error is provided instead, wrapped in an [`Err`].
    pub fn player_or_err(&self) -> Result<&Player, CommandSyntaxError> {
        self.player_or_none()
            .ok_or(REQUIRES_PLAYER.create_without_context())
    }

    /// Returns if the command was executed by a player.
    #[must_use]
    pub fn executed_by_player(&self) -> bool {
        self.player_or_none().is_some()
    }

    /// Sends a message to this source.
    pub fn send_message(&self, message: TextComponent) {
        if !self.silent {
            self.output.send_message(message);
        }
    }

    /// Sends a message to all online operators.
    fn send_to_ops(&self, message: TextComponent) {
        let text = TextComponent::translate_cross(
            "chat.type.admin",
            "chat.type.admin",
            &[self.display_name.clone(), message],
        )
        .color(Color::Named(NamedColor::Gray))
        .italic();
        let Some(server) = &self.server else {
            return;
        };
        if server.level_info.load().game_rules.send_command_feedback {
            let output_player = match &self.output {
                CommandSender::Player(sender) => Some(sender),
                _ => None,
            };
            for player in server.get_all_players() {
                if output_player != Some(&player)
                    && player.permission_lvl.load() >= server.basic_config.op_permission_level
                {
                    player.send_system_message(&text);
                }
            }
        }
    }

    /// Sends feedback to this source.
    pub fn send_feedback(&self, message: TextComponent, broadcast_to_ops: bool) {
        self.send_feedback_with(|| message, broadcast_to_ops);
    }

    /// Like [`Self::send_feedback`], building the message only if it is sent (vanilla
    /// `sendSuccess` with its `Supplier`).
    pub fn send_feedback_with(
        &self,
        message: impl FnOnce() -> TextComponent,
        broadcast_to_ops: bool,
    ) {
        if !self.silent {
            let should_send_to_output = self.output.should_receive_feedback();
            let should_send_to_ops =
                broadcast_to_ops && self.output.should_broadcast_console_to_ops();
            if !should_send_to_output && !should_send_to_ops {
                return;
            }
            let message = message();

            if should_send_to_output {
                self.output.send_message(message.clone());
            }
            if should_send_to_ops {
                self.send_to_ops(message);
            }
        }
    }

    /// Sends an error message to the console.
    ///
    /// # Note
    /// Do not use this function if you want to report a [`CommandSyntaxError`].
    /// Instead, wrap the error in an [`Err`] and return that (or use the `?` operator)
    ///
    /// However, there are still use cases of this function to send an error
    /// without reporting command failure directly.
    pub fn send_error(&self, error: TextComponent) {
        if !self.silent && self.output.should_track_output() {
            self.output.send_message(
                TextComponent::empty()
                    .add_child(error)
                    .color(Color::Named(NamedColor::Red)),
            );
        }
    }

    /// Returns whether this source has the permission provided.
    ///
    /// # Panics
    ///
    /// Panics if this source does not have a reference to the
    /// server (i.e. this is a dummy [`CommandSource`].)
    #[must_use]
    pub fn has_permission(&self, permission: &str) -> bool {
        self.server.as_ref().map_or(
            matches!(
                self.output,
                crate::command::CommandSender::Console | crate::command::CommandSender::Dummy
            ),
            |server| {
                self.permission_level.map_or_else(
                    || self.output.has_permission(server, permission),
                    |level| level_has_permission(server, permission, level),
                )
            },
        )
    }

    /// Returns whether this source has the permission provided.
    ///
    /// # Panics
    ///
    /// Panics **if, and only if** both the following conditions are met:
    ///
    /// - permission is not [`None`].
    /// - this source does not have a reference to the server (i.e. this is a dummy [`CommandSource`].)
    #[must_use]
    pub fn has_permission_from_option(&self, permission: Option<&str>) -> bool {
        permission.is_none_or(|permission| self.has_permission(permission))
    }
}

impl pumpkin_command::source::CommandSource for CommandSource {
    fn send_message(&self, message: TextComponent) {
        self.send_message(message);
    }

    fn send_error(&self, error: TextComponent) {
        self.send_error(error);
    }

    fn call_result(&self, result: ReturnValue) {
        if result.success_value()
            && let crate::command::CommandSender::CommandBlock(block_entity, _) = &self.output
        {
            block_entity
                .success_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.command_result_taker.call(result);
    }

    fn has_permission(&self, permission: &str) -> bool {
        self.has_permission(permission)
    }

    fn position(&self) -> Vector3<f64> {
        self.position
    }

    fn rotation(&self) -> Vector2<f32> {
        self.rotation
    }

    fn check_block_loaded(&self, pos: &BlockPos) -> Result<(), CommandSyntaxError> {
        let world = self.world();
        if world
            .level
            .read_chunk_sync(&pos.chunk_position(), |_| ())
            .is_none()
        {
            Err(
                pumpkin_command::argument_types::coordinates::block_pos::NOT_LOADED_ERROR_TYPE
                    .create_without_context(),
            )
        } else if !world.is_in_build_limit(*pos) {
            Err(
                pumpkin_command::argument_types::coordinates::block_pos::OUT_OF_WORLD_ERROR_TYPE
                    .create_without_context(),
            )
        } else {
            Ok(())
        }
    }

    fn entity_anchor(&self) -> EntityAnchor {
        self.entity_anchor
    }

    fn is_returning(&self) -> bool {
        self.returning
    }

    fn with_frame_return_callback(&self) -> Self {
        self.clone()
            .merge_command_result_taker(&self.frame.return_value)
    }

    fn fall_through(&self) {
        self.frame.return_failure();
        self.frame.discard();
    }

    fn fork_limit(&self) -> usize {
        self.server.as_ref().map_or(usize::MAX, |server| {
            usize::try_from(server.level_info.load().game_rules.max_command_forks).unwrap_or(0)
        })
    }

    fn execute_in_context(&self, run: &mut dyn FnMut()) {
        match &self.server {
            Some(server) => {
                ExecutionContext::execute_in_context(server, |_| run());
            }
            None => run(),
        }
    }

    fn increment_command_cost(&self) {
        if let Some(context) = ExecutionContext::current() {
            context.increment_cost();
        }
    }

    fn command_quota_exhausted(&self) -> bool {
        ExecutionContext::current().is_some_and(|context| context.quota_exhausted())
    }

    fn anchor_position(&self, anchor: EntityAnchor) -> Vector3<f64> {
        let pos = self.position;
        self.entity
            .as_ref()
            .map_or_else(|| pos, |e| anchor.transform_position(pos, e.get_entity()))
    }
}
