use std::sync::MutexGuard;

use crate::command::argument_builder::{
    ArgumentBuilder, LiteralArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::component::ComponentArgumentType;
use crate::command::argument_types::core::bool::BoolArgumentType;
use crate::command::argument_types::core::string::StringArgumentType;
use crate::command::argument_types::score_holder::{ResolvedScoreHolder, ScoreHolderArgumentType};
use crate::command::argument_types::team::TeamArgumentType;
use crate::command::argument_types::team_color::TeamColorArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::REQUIRES_ENTITY;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;
use crate::world::scoreboard::{CollisionRule, NameTagVisibility, Scoreboard, Team};
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

const DESCRIPTION: &str = "Manages teams.";
const PERMISSION: &str = "minecraft:command.team";

const ARG_TEAM_NAME: &str = "team";
const ARG_DISPLAY_NAME: &str = "displayName";
const ARG_TEAM: &str = "team";
const ARG_MEMBERS: &str = "members";
const ARG_VALUE: &str = "value";
const ARG_PREFIX: &str = "prefix";
const ARG_SUFFIX: &str = "suffix";
const ARG_ALLOWED: &str = "allowed";

const DUPLICATE_TEAM_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_ADD_DUPLICATE,
    translation::java::COMMANDS_TEAM_ADD_DUPLICATE,
);

const TEAM_NOT_FOUND_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::TEAM_NOTFOUND,
    translation::java::TEAM_NOTFOUND,
);

const EMPTY_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_EMPTY_UNCHANGED,
    translation::java::COMMANDS_TEAM_EMPTY_UNCHANGED,
);

const COLOR_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_COLOR_UNCHANGED,
    translation::java::COMMANDS_TEAM_OPTION_COLOR_UNCHANGED,
);

const NAME_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_NAME_UNCHANGED,
    translation::java::COMMANDS_TEAM_OPTION_NAME_UNCHANGED,
);

const NAMETAG_VISIBILITY_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_NAMETAGVISIBILITY_UNCHANGED,
    translation::java::COMMANDS_TEAM_OPTION_NAMETAGVISIBILITY_UNCHANGED,
);

const DEATH_MESSAGE_VISIBILITY_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_DEATHMESSAGEVISIBILITY_UNCHANGED,
    translation::java::COMMANDS_TEAM_OPTION_DEATHMESSAGEVISIBILITY_UNCHANGED,
);

const COLLISION_RULE_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_COLLISIONRULE_UNCHANGED,
    translation::java::COMMANDS_TEAM_OPTION_COLLISIONRULE_UNCHANGED,
);

const FRIENDLY_FIRE_ALREADY_ENABLED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADYENABLED,
    translation::java::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADYENABLED,
);

const FRIENDLY_FIRE_ALREADY_DISABLED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADYDISABLED,
    translation::java::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADYDISABLED,
);

const SEE_FRIENDLY_INVISIBLES_ALREADY_ENABLED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_SEEFRIENDLYINVISIBLES_ALREADYENABLED,
    translation::java::COMMANDS_TEAM_OPTION_SEEFRIENDLYINVISIBLES_ALREADYENABLED,
);

const SEE_FRIENDLY_INVISIBLES_ALREADY_DISABLED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_TEAM_OPTION_SEEFRIENDLYINVISIBLES_ALREADYDISABLED,
    translation::java::COMMANDS_TEAM_OPTION_SEEFRIENDLYINVISIBLES_ALREADYDISABLED,
);

fn lock_scoreboard(world: &World) -> MutexGuard<'_, Scoreboard> {
    world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn translated<const N: usize>(key: &'static str, args: [TextComponent; N]) -> TextComponent {
    TextComponent::translate_cross(key, key, args)
}

/// Vanilla `TeamArgument.getTeam`.
fn team_or_error(scoreboard: &Scoreboard, name: &str) -> Result<Team, CommandSyntaxError> {
    scoreboard.get_team(name).cloned().ok_or_else(|| {
        TEAM_NOT_FOUND_ERROR.create_without_context(TextComponent::text(name.to_string()))
    })
}

/// The feedback of vanilla's `CommandResponseTracker` for holders whose membership changed:
/// the holder's name when only one changed, the count otherwise.
fn changed_holders_feedback<const N: usize>(
    changed: &[&ResolvedScoreHolder],
    single_key: &'static str,
    multiple_key: &'static str,
    args: impl FnOnce(TextComponent) -> [TextComponent; N],
) -> TextComponent {
    if let [holder] = changed {
        translated(single_key, args(holder.display_name()))
    } else {
        translated(
            multiple_key,
            args(TextComponent::text(changed.len().to_string())),
        )
    }
}

struct TeamAddExecutor {
    has_display_name: bool,
}

impl CommandExecutor for TeamAddExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let team_name = StringArgumentType::get(context, ARG_TEAM_NAME)?.to_string();
        let display_name = if self.has_display_name {
            ComponentArgumentType::get(context, ARG_DISPLAY_NAME)?
        } else {
            TextComponent::text(team_name.clone())
        };

        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);

        if scoreboard.get_teams().contains_key(&team_name) {
            return Err(DUPLICATE_TEAM_ERROR.create_without_context());
        }

        let new_team = Team {
            name: team_name,
            display_name,
            options: Team::ALLOW_FRIENDLY_FIRE | Team::SEE_FRIENDLY_INVISIBLES,
            nametag_visibility: NameTagVisibility::Always,
            death_message_visibility: NameTagVisibility::Always,
            collision_rule: CollisionRule::Always,
            color: None,
            player_prefix: TextComponent::empty(),
            player_suffix: TextComponent::empty(),
            players: vec![],
        };
        let feedback = translated(
            translation::java::COMMANDS_TEAM_ADD_SUCCESS,
            [new_team.formatted_display_name()],
        );

        scoreboard.add_team(&world, new_team);
        let count = scoreboard.get_teams().len();
        drop(scoreboard);

        context.source.send_feedback(feedback, true);
        Ok(count as i32)
    }
}

struct TeamRemoveExecutor;

impl CommandExecutor for TeamRemoveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let team_name = TeamArgumentType::get(context, ARG_TEAM)?;
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let team = team_or_error(&scoreboard, team_name)?;

        scoreboard.remove_team(&world, team_name);
        let count = scoreboard.get_teams().len();
        drop(scoreboard);

        context.source.send_feedback(
            translated(
                translation::java::COMMANDS_TEAM_REMOVE_SUCCESS,
                [team.formatted_display_name()],
            ),
            true,
        );
        Ok(count as i32)
    }
}

struct TeamEmptyExecutor;

impl CommandExecutor for TeamEmptyExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let team_name = TeamArgumentType::get(context, ARG_TEAM)?;
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let team = team_or_error(&scoreboard, team_name)?;

        if team.players.is_empty() {
            return Err(EMPTY_UNCHANGED_ERROR.create_without_context());
        }
        for player in &team.players {
            scoreboard.remove_player_from_team(&world, team_name, player);
        }
        drop(scoreboard);

        context.source.send_feedback(
            translated(
                translation::java::COMMANDS_TEAM_EMPTY_SUCCESS,
                [
                    TextComponent::text(team.players.len().to_string()),
                    team.formatted_display_name(),
                ],
            ),
            true,
        );
        Ok(team.players.len() as i32)
    }
}

/// The holders `team join`/`leave` act on; without `members`, the executing entity.
fn members(
    context: &CommandContext,
    has_members: bool,
) -> Result<Vec<ResolvedScoreHolder>, CommandSyntaxError> {
    if has_members {
        return ScoreHolderArgumentType::get_score_holders(context, ARG_MEMBERS);
    }
    let entity = context
        .source
        .entity
        .as_ref()
        .ok_or_else(|| REQUIRES_ENTITY.create_without_context())?;
    Ok(vec![ResolvedScoreHolder::entity(entity.clone())])
}

struct TeamJoinExecutor {
    has_members: bool,
}

impl CommandExecutor for TeamJoinExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let team_name = TeamArgumentType::get(context, ARG_TEAM)?;
        let holders = members(context, self.has_members)?;

        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let team = team_or_error(&scoreboard, team_name)?;

        let joined: Vec<&ResolvedScoreHolder> = holders
            .iter()
            .filter(|holder| scoreboard.add_player_to_team(&world, team_name, holder.name.clone()))
            .collect();
        drop(scoreboard);

        context.source.send_feedback(
            changed_holders_feedback(
                &joined,
                translation::java::COMMANDS_TEAM_JOIN_SUCCESS_SINGLE,
                translation::java::COMMANDS_TEAM_JOIN_SUCCESS_MULTIPLE,
                |holder| [holder, team.formatted_display_name()],
            ),
            true,
        );
        Ok(joined.len() as i32)
    }
}

struct TeamLeaveExecutor;

impl CommandExecutor for TeamLeaveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let holders = members(context, true)?;
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);

        let left: Vec<&ResolvedScoreHolder> = holders
            .iter()
            .filter(|holder| {
                let Some(team_name) = scoreboard
                    .get_entity_team(&holder.name)
                    .map(|team| team.name.clone())
                else {
                    return false;
                };
                scoreboard.remove_player_from_team(&world, &team_name, &holder.name)
            })
            .collect();
        drop(scoreboard);

        context.source.send_feedback(
            changed_holders_feedback(
                &left,
                translation::java::COMMANDS_TEAM_LEAVE_SUCCESS_SINGLE,
                translation::java::COMMANDS_TEAM_LEAVE_SUCCESS_MULTIPLE,
                |holder| [holder],
            ),
            true,
        );
        Ok(left.len() as i32)
    }
}

struct TeamListExecutor {
    has_team: bool,
}

impl CommandExecutor for TeamListExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.world().clone();
        let scoreboard = lock_scoreboard(&world);

        let (feedback, count) = if self.has_team {
            let team = team_or_error(&scoreboard, TeamArgumentType::get(context, ARG_TEAM)?)?;
            if team.players.is_empty() {
                (
                    translated(
                        translation::java::COMMANDS_TEAM_LIST_MEMBERS_EMPTY,
                        [team.formatted_display_name()],
                    ),
                    0,
                )
            } else {
                // Vanilla `ComponentUtils.formatList(Collection<String>)` sorts the names and
                // shows them in green.
                let mut members = team.players.clone();
                members.sort();
                (
                    translated(
                        translation::java::COMMANDS_TEAM_LIST_MEMBERS_SUCCESS,
                        [
                            team.formatted_display_name(),
                            TextComponent::text(members.len().to_string()),
                            TextComponent::join_with_comma(
                                members
                                    .into_iter()
                                    .map(|name| {
                                        TextComponent::text(name).color_named(NamedColor::Green)
                                    })
                                    .collect(),
                            ),
                        ],
                    ),
                    team.players.len(),
                )
            }
        } else {
            let teams = scoreboard.get_teams();
            if teams.is_empty() {
                (
                    translated(translation::java::COMMANDS_TEAM_LIST_TEAMS_EMPTY, []),
                    0,
                )
            } else {
                (
                    translated(
                        translation::java::COMMANDS_TEAM_LIST_TEAMS_SUCCESS,
                        [
                            TextComponent::text(teams.len().to_string()),
                            TextComponent::join_with_comma(
                                teams.values().map(Team::formatted_display_name).collect(),
                            ),
                        ],
                    ),
                    teams.len(),
                )
            }
        };
        drop(scoreboard);

        context.source.send_feedback(feedback, false);
        Ok(count as i32)
    }
}

/// The `team modify` option an executor changes.
#[derive(Clone, Copy)]
enum TeamOption {
    ColorReset,
    Color,
    DisplayName,
    Prefix,
    Suffix,
    FriendlyFire,
    SeeFriendlyInvisibles,
    NametagVisibility(NameTagVisibility),
    DeathMessageVisibility(NameTagVisibility),
    CollisionRule(CollisionRule),
}

struct TeamModifyExecutor(TeamOption);

impl TeamModifyExecutor {
    /// Applies the option to `team` and returns the feedback, whether it is broadcast to
    /// operators, and the command result, like vanilla `TeamCommand`'s setters.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per vanilla `team modify` option"
    )]
    fn apply(
        option: TeamOption,
        context: &CommandContext,
        team: &mut Team,
    ) -> Result<(TextComponent, bool, i32), CommandSyntaxError> {
        let (feedback, broadcast, result) = match option {
            TeamOption::ColorReset => {
                if team.color.is_none() {
                    return Err(COLOR_UNCHANGED_ERROR.create_without_context());
                }
                team.color = None;
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_COLOR_CLEAR_SUCCESS,
                        [team.formatted_display_name()],
                    ),
                    true,
                    0,
                )
            }
            TeamOption::Color => {
                let color = TeamColorArgumentType::get(context, ARG_VALUE)?;
                if team.color == Some(color) {
                    return Err(COLOR_UNCHANGED_ERROR.create_without_context());
                }
                team.color = Some(color);
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_COLOR_SUCCESS,
                        [
                            team.formatted_display_name(),
                            TextComponent::text(color.name()),
                        ],
                    ),
                    true,
                    0,
                )
            }
            TeamOption::DisplayName => {
                let display_name = ComponentArgumentType::get(context, ARG_DISPLAY_NAME)?;
                if team.display_name == display_name {
                    return Err(NAME_UNCHANGED_ERROR.create_without_context());
                }
                team.display_name = display_name;
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_NAME_SUCCESS,
                        [team.formatted_display_name()],
                    ),
                    true,
                    0,
                )
            }
            // Vanilla doesn't broadcast prefix and suffix changes and returns 1 for them.
            TeamOption::Prefix => {
                let prefix = ComponentArgumentType::get(context, ARG_PREFIX)?;
                team.player_prefix = prefix.clone();
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_PREFIX_SUCCESS,
                        [prefix],
                    ),
                    false,
                    1,
                )
            }
            TeamOption::Suffix => {
                let suffix = ComponentArgumentType::get(context, ARG_SUFFIX)?;
                team.player_suffix = suffix.clone();
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_SUFFIX_SUCCESS,
                        [suffix],
                    ),
                    false,
                    1,
                )
            }
            TeamOption::FriendlyFire => {
                let allowed = BoolArgumentType::get(context, ARG_ALLOWED)?;
                if team.is_allow_friendly_fire() == allowed {
                    return Err(if allowed {
                        FRIENDLY_FIRE_ALREADY_ENABLED_ERROR.create_without_context()
                    } else {
                        FRIENDLY_FIRE_ALREADY_DISABLED_ERROR.create_without_context()
                    });
                }
                team.options ^= Team::ALLOW_FRIENDLY_FIRE;
                (
                    translated(
                        if allowed {
                            translation::java::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ENABLED
                        } else {
                            translation::java::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_DISABLED
                        },
                        [team.formatted_display_name()],
                    ),
                    true,
                    0,
                )
            }
            TeamOption::SeeFriendlyInvisibles => {
                let allowed = BoolArgumentType::get(context, ARG_ALLOWED)?;
                if (team.options & Team::SEE_FRIENDLY_INVISIBLES != 0) == allowed {
                    return Err(if allowed {
                        SEE_FRIENDLY_INVISIBLES_ALREADY_ENABLED_ERROR.create_without_context()
                    } else {
                        SEE_FRIENDLY_INVISIBLES_ALREADY_DISABLED_ERROR.create_without_context()
                    });
                }
                team.options ^= Team::SEE_FRIENDLY_INVISIBLES;
                (
                    translated(
                        if allowed {
                            translation::java::COMMANDS_TEAM_OPTION_SEEFRIENDLYINVISIBLES_ENABLED
                        } else {
                            translation::java::COMMANDS_TEAM_OPTION_SEEFRIENDLYINVISIBLES_DISABLED
                        },
                        [team.formatted_display_name()],
                    ),
                    true,
                    0,
                )
            }
            TeamOption::NametagVisibility(visibility) => {
                if team.nametag_visibility == visibility {
                    return Err(NAMETAG_VISIBILITY_UNCHANGED_ERROR.create_without_context());
                }
                team.nametag_visibility = visibility;
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_NAMETAGVISIBILITY_SUCCESS,
                        [team.formatted_display_name(), visibility.display_name()],
                    ),
                    true,
                    0,
                )
            }
            TeamOption::DeathMessageVisibility(visibility) => {
                if team.death_message_visibility == visibility {
                    return Err(DEATH_MESSAGE_VISIBILITY_UNCHANGED_ERROR.create_without_context());
                }
                team.death_message_visibility = visibility;
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_DEATHMESSAGEVISIBILITY_SUCCESS,
                        [team.formatted_display_name(), visibility.display_name()],
                    ),
                    true,
                    0,
                )
            }
            TeamOption::CollisionRule(rule) => {
                if team.collision_rule == rule {
                    return Err(COLLISION_RULE_UNCHANGED_ERROR.create_without_context());
                }
                team.collision_rule = rule;
                (
                    translated(
                        translation::java::COMMANDS_TEAM_OPTION_COLLISIONRULE_SUCCESS,
                        [team.formatted_display_name(), rule.display_name()],
                    ),
                    true,
                    0,
                )
            }
        };
        Ok((feedback, broadcast, result))
    }
}

impl CommandExecutor for TeamModifyExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let team_name = TeamArgumentType::get(context, ARG_TEAM)?;
        let world = context.world().clone();
        let mut scoreboard = lock_scoreboard(&world);
        let mut team = team_or_error(&scoreboard, team_name)?;

        let (feedback, broadcast, result) = Self::apply(self.0, context, &mut team)?;
        scoreboard.update_team(&world, team);
        drop(scoreboard);

        context.source.send_feedback(feedback, broadcast);
        Ok(result)
    }
}

fn add_branch() -> LiteralArgumentBuilder {
    literal("add").then(
        argument(ARG_TEAM_NAME, StringArgumentType::SingleWord)
            .executes(TeamAddExecutor {
                has_display_name: false,
            })
            .then(
                argument(ARG_DISPLAY_NAME, ComponentArgumentType).executes(TeamAddExecutor {
                    has_display_name: true,
                }),
            ),
    )
}

fn remove_branch() -> LiteralArgumentBuilder {
    literal("remove").then(argument(ARG_TEAM, TeamArgumentType).executes(TeamRemoveExecutor))
}

fn empty_branch() -> LiteralArgumentBuilder {
    literal("empty").then(argument(ARG_TEAM, TeamArgumentType).executes(TeamEmptyExecutor))
}

fn join_branch() -> LiteralArgumentBuilder {
    literal("join").then(
        argument(ARG_TEAM, TeamArgumentType)
            .executes(TeamJoinExecutor { has_members: false })
            .then(
                argument(ARG_MEMBERS, ScoreHolderArgumentType::Multiple)
                    .executes(TeamJoinExecutor { has_members: true }),
            ),
    )
}

fn leave_branch() -> LiteralArgumentBuilder {
    literal("leave")
        .then(argument(ARG_MEMBERS, ScoreHolderArgumentType::Multiple).executes(TeamLeaveExecutor))
}

fn list_branch() -> LiteralArgumentBuilder {
    literal("list")
        .executes(TeamListExecutor { has_team: false })
        .then(argument(ARG_TEAM, TeamArgumentType).executes(TeamListExecutor { has_team: true }))
}

fn visibility_branch(
    name: &'static str,
    option: fn(NameTagVisibility) -> TeamOption,
) -> LiteralArgumentBuilder {
    literal(name)
        .then(literal("never").executes(TeamModifyExecutor(option(NameTagVisibility::Never))))
        .then(
            literal("hideForOtherTeams").executes(TeamModifyExecutor(option(
                NameTagVisibility::HideForOtherTeams,
            ))),
        )
        .then(
            literal("hideForOwnTeam").executes(TeamModifyExecutor(option(
                NameTagVisibility::HideForOwnTeam,
            ))),
        )
        .then(literal("always").executes(TeamModifyExecutor(option(NameTagVisibility::Always))))
}

fn modify_branch() -> LiteralArgumentBuilder {
    literal("modify").then(
        argument(ARG_TEAM, TeamArgumentType)
            .then(
                literal("displayName").then(
                    argument(ARG_DISPLAY_NAME, ComponentArgumentType)
                        .executes(TeamModifyExecutor(TeamOption::DisplayName)),
                ),
            )
            .then(
                literal("color")
                    .then(literal("reset").executes(TeamModifyExecutor(TeamOption::ColorReset)))
                    .then(
                        argument(ARG_VALUE, TeamColorArgumentType)
                            .executes(TeamModifyExecutor(TeamOption::Color)),
                    ),
            )
            .then(
                literal("friendlyFire").then(
                    argument(ARG_ALLOWED, BoolArgumentType)
                        .executes(TeamModifyExecutor(TeamOption::FriendlyFire)),
                ),
            )
            .then(
                literal("seeFriendlyInvisibles").then(
                    argument(ARG_ALLOWED, BoolArgumentType)
                        .executes(TeamModifyExecutor(TeamOption::SeeFriendlyInvisibles)),
                ),
            )
            .then(visibility_branch(
                "nametagVisibility",
                TeamOption::NametagVisibility,
            ))
            .then(visibility_branch(
                "deathMessageVisibility",
                TeamOption::DeathMessageVisibility,
            ))
            .then(
                literal("collisionRule")
                    .then(
                        literal("never").executes(TeamModifyExecutor(TeamOption::CollisionRule(
                            CollisionRule::Never,
                        ))),
                    )
                    .then(literal("pushOwnTeam").executes(TeamModifyExecutor(
                        TeamOption::CollisionRule(CollisionRule::PushOwnTeam),
                    )))
                    .then(literal("pushOtherTeams").executes(TeamModifyExecutor(
                        TeamOption::CollisionRule(CollisionRule::PushOtherTeams),
                    )))
                    .then(literal("always").executes(TeamModifyExecutor(
                        TeamOption::CollisionRule(CollisionRule::Always),
                    ))),
            )
            .then(
                literal("prefix").then(
                    argument(ARG_PREFIX, ComponentArgumentType)
                        .executes(TeamModifyExecutor(TeamOption::Prefix)),
                ),
            )
            .then(
                literal("suffix").then(
                    argument(ARG_SUFFIX, ComponentArgumentType)
                        .executes(TeamModifyExecutor(TeamOption::Suffix)),
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
        command("team", DESCRIPTION)
            .requires(PERMISSION)
            .then(list_branch())
            .then(add_branch())
            .then(remove_branch())
            .then(empty_branch())
            .then(join_branch())
            .then(leave_branch())
            .then(modify_branch()),
    );
}
