use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::core::bool::BoolArgumentType;
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::resource::{MOB_EFFECT_ARGUMENT, ResourceArgument};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::EntityBase;

const DESCRIPTION: &str = "Adds or removes the status effects of players and other entities.";
const PERMISSION: &str = "minecraft:command.effect";

const ERROR_GIVE_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_EFFECT_GIVE_FAILED,
    translation::java::COMMANDS_EFFECT_GIVE_FAILED,
);

const ERROR_CLEAR_EVERYTHING_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_EFFECT_CLEAR_EVERYTHING_FAILED,
    translation::java::COMMANDS_EFFECT_CLEAR_EVERYTHING_FAILED,
);

const ERROR_CLEAR_SPECIFIC_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_EFFECT_CLEAR_SPECIFIC_FAILED,
    translation::java::COMMANDS_EFFECT_CLEAR_SPECIFIC_FAILED,
);

/// Vanilla `CommandResponseTracker` with `ElementType.NON_ZERO`: the feedback names the only
/// entity that changed, or counts the entities that changed.
#[derive(Default)]
struct ResponseTracker {
    changed: i32,
    only_changed: Option<TextComponent>,
}

impl ResponseTracker {
    fn track(&mut self, entity: &dyn EntityBase, changed: bool) {
        if !changed {
            return;
        }
        self.changed += 1;
        self.only_changed = (self.changed == 1).then(|| entity.get_display_name());
    }

    fn send_feedback(
        self,
        context: &CommandContext,
        failed: &'static CommandErrorType<0>,
        single_key: &'static str,
        multiple_key: &'static str,
        mut args: Vec<TextComponent>,
    ) -> CommandExecutorResult {
        if self.changed == 0 {
            return Err(failed.create_without_context());
        }
        let (key, target) = match self.only_changed {
            Some(name) => (single_key, name),
            None => (multiple_key, TextComponent::text(self.changed.to_string())),
        };
        args.push(target);
        context
            .source
            .send_feedback(TextComponent::translate_cross(key, key, args), true);
        Ok(self.changed)
    }
}

fn effect_name(effect: &StatusEffect) -> TextComponent {
    TextComponent::translate_cross(effect.translation_key, effect.translation_key, [])
}

#[derive(Clone, Copy)]
enum Duration {
    Default,
    Specified,
    Infinite,
}

struct GiveExecutor {
    duration: Duration,
    has_amplifier: bool,
    has_hide_particles: bool,
}

impl CommandExecutor for GiveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let targets = EntityArgumentType::get_entities(context, "targets")?;
        let effect = ResourceArgument::get_mob_effect(context, "effect")?;

        // Vanilla `EffectCommands.computeDurationInTicks`: instant effects take the seconds
        // argument as ticks.
        let instant = effect.instantaneous;
        let duration_ticks = match self.duration {
            Duration::Default if instant => 1,
            Duration::Default => 30 * 20,
            Duration::Specified if instant => IntegerArgumentType::get(context, "seconds")?,
            Duration::Specified => IntegerArgumentType::get(context, "seconds")? * 20,
            Duration::Infinite => -1,
        };

        let amplifier = if self.has_amplifier {
            IntegerArgumentType::get(context, "amplifier")? as u8
        } else {
            0
        };

        let hide_particles = if self.has_hide_particles {
            BoolArgumentType::get(context, "hideParticles")?
        } else {
            false
        };

        let mut tracker = ResponseTracker::default();
        for target in &targets {
            let Some(living) = target.get_living_entity() else {
                continue;
            };
            let changed = living.add_effect_with_source(
                Effect {
                    effect_type: effect,
                    duration: duration_ticks,
                    amplifier,
                    ambient: false,
                    show_particles: !hide_particles,
                    show_icon: true,
                    blend: false,
                },
                context.source.entity.as_deref(),
            );
            tracker.track(target.as_ref(), changed);
        }

        tracker.send_feedback(
            context,
            &ERROR_GIVE_FAILED,
            translation::java::COMMANDS_EFFECT_GIVE_SUCCESS_SINGLE,
            translation::java::COMMANDS_EFFECT_GIVE_SUCCESS_MULTIPLE,
            vec![effect_name(effect)],
        )
    }
}

enum ClearMode {
    SelfAll,
    TargetsAll,
    TargetsSpecific,
}

struct ClearExecutor(ClearMode);

impl CommandExecutor for ClearExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let targets = match self.0 {
            ClearMode::SelfAll => vec![context.source.entity_or_err()?],
            ClearMode::TargetsAll | ClearMode::TargetsSpecific => {
                EntityArgumentType::get_entities(context, "targets")?
            }
        };

        let mut tracker = ResponseTracker::default();
        if matches!(self.0, ClearMode::TargetsSpecific) {
            let effect = ResourceArgument::get_mob_effect(context, "effect")?;
            for target in &targets {
                if let Some(living) = target.get_living_entity() {
                    tracker.track(target.as_ref(), living.remove_effect(effect));
                }
            }
            return tracker.send_feedback(
                context,
                &ERROR_CLEAR_SPECIFIC_FAILED,
                translation::java::COMMANDS_EFFECT_CLEAR_SPECIFIC_SUCCESS_SINGLE,
                translation::java::COMMANDS_EFFECT_CLEAR_SPECIFIC_SUCCESS_MULTIPLE,
                vec![effect_name(effect)],
            );
        }

        for target in &targets {
            if let Some(living) = target.get_living_entity() {
                tracker.track(target.as_ref(), living.remove_all_effects());
            }
        }
        tracker.send_feedback(
            context,
            &ERROR_CLEAR_EVERYTHING_FAILED,
            translation::java::COMMANDS_EFFECT_CLEAR_EVERYTHING_SUCCESS_SINGLE,
            translation::java::COMMANDS_EFFECT_CLEAR_EVERYTHING_SUCCESS_MULTIPLE,
            Vec::new(),
        )
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let seconds_node = argument("seconds", IntegerArgumentType::new(1, 1_000_000))
        .executes(GiveExecutor {
            duration: Duration::Specified,
            has_amplifier: false,
            has_hide_particles: false,
        })
        .then(
            argument("amplifier", IntegerArgumentType::new(0, 255))
                .executes(GiveExecutor {
                    duration: Duration::Specified,
                    has_amplifier: true,
                    has_hide_particles: false,
                })
                .then(
                    argument("hideParticles", BoolArgumentType).executes(GiveExecutor {
                        duration: Duration::Specified,
                        has_amplifier: true,
                        has_hide_particles: true,
                    }),
                ),
        );

    let infinite_node = literal("infinite")
        .executes(GiveExecutor {
            duration: Duration::Infinite,
            has_amplifier: false,
            has_hide_particles: false,
        })
        .then(
            argument("amplifier", IntegerArgumentType::new(0, 255))
                .executes(GiveExecutor {
                    duration: Duration::Infinite,
                    has_amplifier: true,
                    has_hide_particles: false,
                })
                .then(
                    argument("hideParticles", BoolArgumentType).executes(GiveExecutor {
                        duration: Duration::Infinite,
                        has_amplifier: true,
                        has_hide_particles: true,
                    }),
                ),
        );

    let give_node = literal("give").then(
        argument("targets", EntityArgumentType::Entities).then(
            argument("effect", MOB_EFFECT_ARGUMENT.clone())
                .executes(GiveExecutor {
                    duration: Duration::Default,
                    has_amplifier: false,
                    has_hide_particles: false,
                })
                .then(seconds_node)
                .then(infinite_node),
        ),
    );

    let clear_node = literal("clear")
        .executes(ClearExecutor(ClearMode::SelfAll))
        .then(
            argument("targets", EntityArgumentType::Entities)
                .executes(ClearExecutor(ClearMode::TargetsAll))
                .then(
                    argument("effect", MOB_EFFECT_ARGUMENT.clone())
                        .executes(ClearExecutor(ClearMode::TargetsSpecific)),
                ),
        );

    dispatcher.register(
        command("effect", DESCRIPTION)
            .requires(PERMISSION)
            .then(clear_node)
            .then(give_node),
    );
}
