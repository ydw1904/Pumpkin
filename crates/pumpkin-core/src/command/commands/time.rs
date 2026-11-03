use pumpkin_data::environment_attribute::{TIMELINES, TimeMarker, Timeline};
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::float::FloatArgumentType;
use crate::command::argument_types::identifier::IdentifierArgumentType;
use crate::command::argument_types::registry_id::RegistryIdArgumentType;
use crate::command::argument_types::time::TimeArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use crate::command::suggestion::suggestions::SuggestionsBuilder;
use crate::world::World;
use pumpkin_util::identifier::Identifier;
use std::sync::Arc;

const DESCRIPTION: &str = "Query or modify the world time and clocks.";
const PERMISSION: &str = "minecraft:command.time";

const TIMELINE_REGISTRY: &str = "minecraft:timeline";

static ERROR_NO_DEFAULT_CLOCK: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_NO_DEFAULT_CLOCK,
    translation::java::COMMANDS_TIME_NO_DEFAULT_CLOCK,
);
static ERROR_UNKNOWN_TIMELINE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_NOT_FOUND,
    translation::java::ARGUMENT_RESOURCE_NOT_FOUND,
);
static ERROR_NO_TIME_MARKER_FOUND: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_NO_TIME_MARKER_FOUND,
    translation::java::COMMANDS_TIME_NO_TIME_MARKER_FOUND,
);
static ERROR_WRONG_TIMELINE_FOR_CLOCK: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_WRONG_TIMELINE_FOR_CLOCK,
    translation::java::COMMANDS_TIME_WRONG_TIMELINE_FOR_CLOCK,
);
static ERROR_ALREADY_AT_TIME_MARKER: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_SET_ALREADY_AT_TIME_MARKER,
    "The clock is already at that time marker",
);
static ERROR_ALREADY_AT_TIME: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_SET_ALREADY_AT_TIME,
    "The clock is already at that time",
);
static ERROR_ALREADY_PAUSED: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_PAUSE_ALREADY_PAUSED,
    translation::java::COMMANDS_TIME_PAUSE_ALREADY_PAUSED,
);
static ERROR_ALREADY_RUNNING: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_PAUSE_ALREADY_RUNNING,
    translation::java::COMMANDS_TIME_PAUSE_ALREADY_RUNNING,
);
static ERROR_ALREADY_SAME_RATE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_TIME_RATE_ALREADY_SAME,
    translation::java::COMMANDS_TIME_RATE_ALREADY_SAME,
);

#[derive(Clone, Copy)]
enum Action {
    SetTicks,
    SetTimeMarker,
    Add,
    Pause,
    Resume,
    Rate,
}

#[derive(Clone, Copy)]
enum QueryMode {
    Time,
    GameTime,
    TimelineTicks,
    TimelineRepetitions,
}

const fn wrap_time(ticks: i64) -> i32 {
    (ticks % 2_147_483_647) as i32
}

/// The `of <clock>` argument, or vanilla `TimeCommand.getDefaultClock` for the source's dimension.
fn clock_name(context: &CommandContext, has_clock: bool) -> Result<String, CommandSyntaxError> {
    if has_clock {
        return Ok(IdentifierArgumentType::get(context, "clock")?.to_string());
    }
    let dimension = &context.source.world().dimension;
    dimension.default_clock.map(str::to_string).ok_or_else(|| {
        ERROR_NO_DEFAULT_CLOCK.create_without_context(TextComponent::text(dimension.minecraft_name))
    })
}

/// The clock and the world that keeps its time: the one whose dimension uses it by default.
fn resolve_clock(
    context: &CommandContext,
    has_clock: bool,
) -> Result<(String, Arc<World>), CommandSyntaxError> {
    let clock = clock_name(context, has_clock)?;
    let world = context
        .server()
        .worlds
        .load()
        .iter()
        .find(|world| world.dimension.default_clock == Some(clock.as_str()))
        .cloned()
        .unwrap_or_else(|| context.source.world().clone());
    Ok((clock, world))
}

/// The `<timeline>` argument, which has to run on `clock`.
fn timeline_for_clock(
    context: &CommandContext,
    clock: &str,
) -> Result<&'static Timeline, CommandSyntaxError> {
    let id = RegistryIdArgumentType::get(context, "timeline")?.to_string();
    let Some(timeline) = TIMELINES.iter().find(|timeline| timeline.name == id) else {
        return Err(ERROR_UNKNOWN_TIMELINE.create_without_context(
            TextComponent::text(id),
            TextComponent::text(TIMELINE_REGISTRY),
        ));
    };
    if timeline.clock != clock {
        return Err(ERROR_WRONG_TIMELINE_FOR_CLOCK.create_without_context(
            TextComponent::text(timeline.name),
            TextComponent::text(clock.to_string()),
        ));
    }
    Ok(timeline)
}

/// The time markers of the timelines on `clock`, with their timeline's period.
fn clock_time_markers(clock: &str) -> impl Iterator<Item = (&'static TimeMarker, Option<i64>)> {
    TIMELINES
        .iter()
        .filter(move |timeline| timeline.clock == clock)
        .flat_map(|timeline| {
            timeline
                .time_markers
                .iter()
                .map(|marker| (marker, timeline.period_ticks.map(i64::from)))
        })
}

/// Timeline feedback; Bedrock has its own one-argument keys.
fn timeline_feedback(
    context: &CommandContext,
    java_key: &'static str,
    bedrock_key: &'static str,
    timeline: &Timeline,
    value: i64,
) -> TextComponent {
    let value = TextComponent::text(value.to_string());
    let is_bedrock = context.source.output.as_player().is_some_and(|player| {
        matches!(
            player.client.as_ref(),
            crate::net::ClientPlatform::Bedrock(_)
        )
    });
    if is_bedrock {
        TextComponent::translate_cross(java_key, bedrock_key, [value])
    } else {
        TextComponent::translate_cross(
            java_key,
            bedrock_key,
            [TextComponent::text(timeline.name), value],
        )
    }
}

struct QueryExecutor {
    mode: QueryMode,
    has_clock: bool,
}

impl CommandExecutor for QueryExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        // `query gametime` reads the source's world and needs no clock.
        let (clock_name, world) = if matches!(self.mode, QueryMode::GameTime) {
            (String::new(), context.source.world().clone())
        } else {
            resolve_clock(context, self.has_clock)?
        };
        let level_time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        match self.mode {
            QueryMode::GameTime => {
                let game_time = level_time.query_gametime();
                context.source.send_feedback(
                    pumpkin_macros::translate_cross!(
                        translation::java::COMMANDS_TIME_QUERY_GAMETIME,
                        translation::bedrock::COMMANDS_TIME_QUERY_GAMETIME,
                        TextComponent::text(game_time.to_string())
                    ),
                    false,
                );
                Ok(wrap_time(game_time))
            }
            QueryMode::Time => {
                let total_ticks = level_time.time_of_day;
                context.source.send_feedback(
                    pumpkin_macros::translate_cross!(
                        translation::java::COMMANDS_TIME_QUERY_ABSOLUTE,
                        translation::bedrock::COMMANDS_TIME_QUERY_DAYTIME,
                        TextComponent::text(clock_name),
                        TextComponent::text(total_ticks.to_string())
                    ),
                    false,
                );
                Ok(wrap_time(total_ticks))
            }
            // Vanilla `Timeline.getCurrentTicks`.
            QueryMode::TimelineTicks => {
                let timeline = timeline_for_clock(context, &clock_name)?;
                let total_ticks = level_time.time_of_day;
                let ticks = timeline
                    .period_ticks
                    .map_or(total_ticks, |period| total_ticks % i64::from(period));
                context.source.send_feedback(
                    timeline_feedback(
                        context,
                        translation::java::COMMANDS_TIME_QUERY_TIMELINE,
                        translation::bedrock::COMMANDS_TIME_QUERY_DAYTIME,
                        timeline,
                        ticks,
                    ),
                    false,
                );
                Ok(wrap_time(ticks))
            }
            // Vanilla `Timeline.getPeriodCount`.
            QueryMode::TimelineRepetitions => {
                let timeline = timeline_for_clock(context, &clock_name)?;
                let repetitions = timeline
                    .period_ticks
                    .map_or(0, |period| level_time.time_of_day / i64::from(period));
                context.source.send_feedback(
                    timeline_feedback(
                        context,
                        translation::java::COMMANDS_TIME_QUERY_TIMELINE_REPETITIONS,
                        translation::bedrock::COMMANDS_TIME_QUERY_DAY,
                        timeline,
                        repetitions,
                    ),
                    false,
                );
                Ok(wrap_time(repetitions))
            }
        }
    }
}

struct ActionExecutor {
    action: Action,
    has_clock: bool,
}

impl CommandExecutor for ActionExecutor {
    #[allow(clippy::too_many_lines)]
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let (clock_name, world) = resolve_clock(context, self.has_clock)?;
        let world = &world;

        match self.action {
            Action::SetTimeMarker => {
                let marker_id = IdentifierArgumentType::get(context, "timemarker")?.to_string();
                let Some((marker, period)) =
                    clock_time_markers(&clock_name).find(|(marker, _)| marker.name == marker_id)
                else {
                    return Err(ERROR_NO_TIME_MARKER_FOUND.create_without_context(
                        TextComponent::text(marker_id),
                        TextComponent::text(clock_name),
                    ));
                };
                // Vanilla `ClockTimeMarker.occursAt` and `resolveTimeToMoveTo`: periodic markers
                // move forward to their next occurrence, so the day count keeps going.
                let marker_ticks = i64::from(marker.ticks);
                let moved = {
                    let mut guard = world
                        .level_time
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let current = guard.time_of_day;
                    let target = match period {
                        Some(period) if current % period != marker_ticks => {
                            let duration = marker_ticks - current % period;
                            Some(
                                current
                                    + if duration > 0 {
                                        duration
                                    } else {
                                        period + duration
                                    },
                            )
                        }
                        None if current != marker_ticks => Some(marker_ticks),
                        _ => None,
                    };
                    target.inspect(|&target| guard.set_time(target))
                };
                let Some(total_ticks) = moved else {
                    return Err(ERROR_ALREADY_AT_TIME_MARKER.create_without_context(
                        TextComponent::text(clock_name),
                        TextComponent::text(marker.name),
                    ));
                };
                world.clock_changed();
                context.source.send_feedback(
                    pumpkin_macros::translate_cross!(
                        translation::java::COMMANDS_TIME_SET_TIME_MARKER,
                        translation::bedrock::COMMANDS_TIME_SET,
                        TextComponent::text(clock_name),
                        TextComponent::text(marker.name)
                    ),
                    true,
                );
                Ok(wrap_time(total_ticks))
            }
            Action::SetTicks => {
                let time_count = TimeArgumentType::get(context, "time")?;
                let mut guard = world
                    .level_time
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if guard.time_of_day == i64::from(time_count) {
                    return Err(ERROR_ALREADY_AT_TIME.create_without_context(
                        TextComponent::text(clock_name),
                        TextComponent::text(time_count.to_string()),
                    ));
                }
                guard.set_time(time_count.into());
                drop(guard);
                world.clock_changed();
                context.source.send_feedback(
                    pumpkin_macros::translate_cross!(
                        translation::java::COMMANDS_TIME_SET_ABSOLUTE,
                        translation::bedrock::COMMANDS_TIME_SET,
                        TextComponent::text(clock_name),
                        TextComponent::text(time_count.to_string())
                    ),
                    true,
                );
                Ok(time_count)
            }
            Action::Add => {
                let time_count = TimeArgumentType::get(context, "time")?;
                let total_ticks = {
                    let mut guard = world
                        .level_time
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.add_time(time_count.into());
                    guard.time_of_day
                };
                world.clock_changed();
                context.source.send_feedback(
                    pumpkin_macros::translate_cross!(
                        translation::java::COMMANDS_TIME_SET_ABSOLUTE,
                        translation::bedrock::COMMANDS_TIME_ADDED,
                        TextComponent::text(clock_name),
                        TextComponent::text(total_ticks.to_string())
                    ),
                    true,
                );
                Ok(wrap_time(total_ticks))
            }
            Action::Pause | Action::Resume => {
                let paused = matches!(self.action, Action::Pause);
                let mut guard = world
                    .level_time
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if guard.paused == paused {
                    let error = if paused {
                        &ERROR_ALREADY_PAUSED
                    } else {
                        &ERROR_ALREADY_RUNNING
                    };
                    return Err(error.create_without_context(TextComponent::text(clock_name)));
                }
                guard.set_paused(paused);
                drop(guard);
                world.clock_changed();
                context.source.send_feedback(
                    if paused {
                        pumpkin_macros::translate_cross!(
                            translation::java::COMMANDS_TIME_PAUSE,
                            translation::bedrock::COMMANDS_TIME_STOP,
                            TextComponent::text(clock_name)
                        )
                    } else {
                        pumpkin_macros::translate_cross!(
                            translation::java::COMMANDS_TIME_RESUME,
                            translation::bedrock::COMMANDS_TIME_SET,
                            TextComponent::text(clock_name)
                        )
                    },
                    true,
                );
                Ok(1)
            }
            Action::Rate => {
                let rate = FloatArgumentType::get(context, "rate")?;
                let mut guard = world
                    .level_time
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                #[expect(clippy::float_cmp, reason = "vanilla compares the rates exactly")]
                if guard.rate == rate {
                    return Err(ERROR_ALREADY_SAME_RATE.create_without_context(
                        TextComponent::text(clock_name),
                        TextComponent::text(rate.to_string()),
                    ));
                }
                guard.set_rate(rate);
                drop(guard);
                world.clock_changed();
                context.source.send_feedback(
                    pumpkin_macros::translate_cross!(
                        translation::java::COMMANDS_TIME_RATE,
                        translation::bedrock::COMMANDS_TIME_SET,
                        TextComponent::text(clock_name),
                        TextComponent::text(rate.to_string())
                    ),
                    true,
                );
                Ok(1)
            }
        }
    }
}

/// Vanilla `TimeCommand.suggestTimeMarkers`: the markers shown in commands for the clock.
struct TimeMarkerSuggestionProvider {
    has_clock: bool,
}

impl SuggestionProvider for TimeMarkerSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let Ok(clock) = clock_name(context, self.has_clock) else {
            return builder.build();
        };
        let markers: Vec<&str> = clock_time_markers(&clock)
            .filter(|(marker, _)| marker.show_in_commands)
            .map(|(marker, _)| marker.name)
            .collect();
        builder.filter_and_suggest_iter(markers).build()
    }
}

macro_rules! build_branches {
    ($builder:expr, $has_clock:expr) => {{
        let set_node = literal("set")
            .then(
                argument("time", TimeArgumentType::any()).executes(ActionExecutor {
                    action: Action::SetTicks,
                    has_clock: $has_clock,
                }),
            )
            .then(
                argument("timemarker", IdentifierArgumentType)
                    .suggests(TimeMarkerSuggestionProvider {
                        has_clock: $has_clock,
                    })
                    .executes(ActionExecutor {
                        action: Action::SetTimeMarker,
                        has_clock: $has_clock,
                    }),
            );

        let add_node = literal("add").then(
            argument("time", TimeArgumentType::new(i32::MIN)).executes(ActionExecutor {
                action: Action::Add,
                has_clock: $has_clock,
            }),
        );

        let pause_node = literal("pause").executes(ActionExecutor {
            action: Action::Pause,
            has_clock: $has_clock,
        });
        let resume_node = literal("resume").executes(ActionExecutor {
            action: Action::Resume,
            has_clock: $has_clock,
        });

        let rate_node = literal("rate").then(
            argument("rate", FloatArgumentType::new(1.0e-5, 1000.0)).executes(ActionExecutor {
                action: Action::Rate,
                has_clock: $has_clock,
            }),
        );

        let query_node = literal("query")
            .then(literal("time").executes(QueryExecutor {
                mode: QueryMode::Time,
                has_clock: $has_clock,
            }))
            .then(
                argument(
                    "timeline",
                    RegistryIdArgumentType::resource(Identifier::vanilla_static("timeline")),
                )
                .executes(QueryExecutor {
                    mode: QueryMode::TimelineTicks,
                    has_clock: $has_clock,
                })
                .then(literal("repetition").executes(QueryExecutor {
                    mode: QueryMode::TimelineRepetitions,
                    has_clock: $has_clock,
                })),
            );

        // `query gametime` only exists without `of <clock>`.
        let query_node = if $has_clock {
            query_node
        } else {
            query_node.then(literal("gametime").executes(QueryExecutor {
                mode: QueryMode::GameTime,
                has_clock: false,
            }))
        };

        $builder
            .then(set_node)
            .then(add_node)
            .then(pause_node)
            .then(resume_node)
            .then(rate_node)
            .then(query_node)
    }};
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let of_clock_node = literal("of").then(build_branches!(
        argument(
            "clock",
            RegistryIdArgumentType::resource(Identifier::vanilla_static("world_clock"))
        ),
        true
    ));

    let time_cmd = build_branches!(command("time", DESCRIPTION).requires(PERMISSION), false)
        .then(of_clock_node);

    dispatcher.register(time_cmd);
}
