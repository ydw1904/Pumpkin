use crate::command::{
    CommandSource,
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    context::command_context::CommandContext,
    errors::command_syntax_error::CommandSyntaxError,
    errors::error_types::CommandErrorType,
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};
use crate::world::scoreboard::{ALL_DISPLAY_SLOTS, display_slot_name};
use pumpkin_data::scoreboard::ScoreboardDisplaySlot;
use pumpkin_data::translation;
use pumpkin_util::text::TextComponent;

const INVALID_SLOT_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_SCOREBOARDDISPLAYSLOT_INVALID,
    translation::java::ARGUMENT_SCOREBOARDDISPLAYSLOT_INVALID,
);

/// Vanilla `ScoreboardSlotArgument`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ScoreboardSlotArgumentType;

impl ArgumentType<CommandSource> for ScoreboardSlotArgumentType {
    type Item = ScoreboardDisplaySlot;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let name = reader.read_unquoted_string();
        ALL_DISPLAY_SLOTS
            .into_iter()
            .find(|slot| display_slot_name(*slot) == name)
            .ok_or_else(|| INVALID_SLOT_ERROR.create(reader, TextComponent::text(name)))
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::ScoreboardSlot
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> Suggestions {
        for slot in ALL_DISPLAY_SLOTS {
            builder = builder.filter_and_suggest_one(display_slot_name(slot));
        }
        builder.build()
    }

    fn examples(&self) -> Vec<String> {
        vec!["sidebar".to_string(), "foo.bar".to_string()]
    }
}

impl ScoreboardSlotArgumentType {
    pub fn get(
        context: &CommandContext,
        name: &str,
    ) -> Result<ScoreboardDisplaySlot, CommandSyntaxError> {
        context.get_argument::<ScoreboardDisplaySlot>(name).copied()
    }
}
