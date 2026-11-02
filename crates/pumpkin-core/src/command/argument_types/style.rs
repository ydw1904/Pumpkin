use crate::command::{
    CommandSource,
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    argument_types::nbt::NbtTagArgumentType,
    context::command_context::CommandContext,
    errors::command_syntax_error::CommandSyntaxError,
    errors::error_types::CommandErrorType,
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};
use crate::world::scoreboard::style_from_nbt;
use pumpkin_data::translation;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::{TextComponent, style::Style};

const INVALID_STYLE_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_STYLE_INVALID,
    translation::java::ARGUMENT_STYLE_INVALID,
);

/// Vanilla `StyleArgument`: a style written as SNBT, like `{color:'red',bold:true}`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct StyleArgumentType;

impl ArgumentType<CommandSource> for StyleArgumentType {
    type Item = Style;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        match <NbtTagArgumentType as ArgumentType<CommandSource>>::parse(
            &NbtTagArgumentType,
            reader,
        )? {
            NbtTag::Compound(compound) => Ok(style_from_nbt(&compound)),
            tag => Err(INVALID_STYLE_ERROR
                .create(reader, TextComponent::text(format!("Not a map: {tag:?}")))),
        }
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::Style
    }

    fn list_suggestions(
        &self,
        context: &CommandContext,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        NbtTagArgumentType.list_suggestions(context, builder)
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "{bold: true}".to_string(),
            "{color: 'red'}".to_string(),
            "{}".to_string(),
        ]
    }
}

impl StyleArgumentType {
    pub fn get(context: &CommandContext, name: &str) -> Result<Style, CommandSyntaxError> {
        context.get_argument::<Style>(name).cloned()
    }
}
