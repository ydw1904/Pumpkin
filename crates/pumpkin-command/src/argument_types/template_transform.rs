use crate::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::context::command_context::CommandContext;
use crate::errors::command_syntax_error::CommandSyntaxError;
use crate::errors::error_types::CommandErrorType;
use crate::string_reader::StringReader;
use crate::suggestion::suggestions::{Suggestions, SuggestionsBuilder};
use pumpkin_data::translation;
use pumpkin_data::{Mirror, Rotation};
use pumpkin_util::text::TextComponent;

/// Vanilla `StringRepresentableArgument.ERROR_INVALID_VALUE`.
pub const INVALID_ERROR_TYPE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_ENUM_INVALID,
    translation::java::ARGUMENT_ENUM_INVALID,
);

const ROTATIONS: [(&str, Rotation); 4] = [
    ("none", Rotation::None),
    ("clockwise_90", Rotation::Clockwise90),
    ("180", Rotation::Rotate180),
    ("counterclockwise_90", Rotation::CounterClockwise90),
];

const MIRRORS: [(&str, Mirror); 3] = [
    ("none", Mirror::None),
    ("left_right", Mirror::LeftRight),
    ("front_back", Mirror::FrontBack),
];

/// Vanilla `StringRepresentableArgument.parse`.
fn parse_value<T: Copy>(
    reader: &mut StringReader,
    values: &[(&str, T)],
) -> Result<T, CommandSyntaxError> {
    let name = reader.read_unquoted_string();
    values
        .iter()
        .find_map(|(id, value)| (*id == name).then_some(*value))
        .ok_or_else(|| INVALID_ERROR_TYPE.create(reader, TextComponent::text(name)))
}

/// Vanilla `TemplateRotationArgument`.
pub struct TemplateRotationArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for TemplateRotationArgumentType {
    type Item = Rotation;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        parse_value(reader, &ROTATIONS)
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::TemplateRotation
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder
            .filter_and_suggest(&ROTATIONS.map(|(id, _)| id))
            .build()
    }

    fn examples(&self) -> Vec<String> {
        examples!("none", "clockwise_90")
    }
}

impl_copy_get!(TemplateRotationArgumentType, Rotation);

/// Vanilla `TemplateMirrorArgument`.
pub struct TemplateMirrorArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for TemplateMirrorArgumentType {
    type Item = Mirror;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        parse_value(reader, &MIRRORS)
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::TemplateMirror
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder
            .filter_and_suggest(&MIRRORS.map(|(id, _)| id))
            .build()
    }

    fn examples(&self) -> Vec<String> {
        examples!("none", "left_right")
    }
}

impl_copy_get!(TemplateMirrorArgumentType, Mirror);
