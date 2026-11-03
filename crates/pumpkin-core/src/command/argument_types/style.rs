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
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::{TextComponent, color::NamedColor, style::Style};

const INVALID_STYLE_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_STYLE_INVALID,
    translation::java::ARGUMENT_STYLE_INVALID,
);

/// The field checks of vanilla `Style.Serializer.MAP_CODEC` that the lenient component reader
/// skips, with `NbtOps` and `TextColor.parseColor` error messages.
fn validate_style(nbt: &NbtCompound) -> Result<(), String> {
    if let Some(tag) = nbt.get("color") {
        let color = tag.extract_string().ok_or("Not a string")?;
        if let Some(hex) = color.strip_prefix('#') {
            let value = i32::from_str_radix(hex, 16)
                .map_err(|_| format!("Invalid color value: {color}"))?;
            if !(0..=0xFF_FFFF).contains(&value) {
                return Err(format!("Color value out of range: {color}"));
            }
        } else if NamedColor::try_from(color).is_err() {
            return Err(format!("Invalid color name: {color}"));
        }
    }
    for key in [
        "bold",
        "italic",
        "underlined",
        "strikethrough",
        "obfuscated",
    ] {
        if let Some(tag) = nbt.get(key)
            && !matches!(
                tag,
                NbtTag::Byte(_)
                    | NbtTag::Short(_)
                    | NbtTag::Int(_)
                    | NbtTag::Long(_)
                    | NbtTag::Float(_)
                    | NbtTag::Double(_)
            )
        {
            return Err("Not a number".to_string());
        }
    }
    for key in ["insertion", "font"] {
        if nbt
            .get(key)
            .is_some_and(|tag| tag.extract_string().is_none())
        {
            return Err("Not a string".to_string());
        }
    }
    Ok(())
}

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
            NbtTag::Compound(compound) => match validate_style(&compound) {
                Ok(()) => Ok(style_from_nbt(&compound)),
                Err(message) => {
                    Err(INVALID_STYLE_ERROR.create(reader, TextComponent::text(message)))
                }
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_styles_the_vanilla_codec_rejects() {
        for input in [
            "{color:'nonsense'}",
            "{color:'#zzzzzz'}",
            "{color:'#1000000'}",
            "{bold:'yes'}",
        ] {
            let mut reader = StringReader::new(input);
            let error = StyleArgumentType.parse(&mut reader).unwrap_err();
            assert_eq!(
                serde_json::to_value(error.message).unwrap()["translate"],
                translation::java::ARGUMENT_STYLE_INVALID,
                "{input}"
            );
        }
        let mut reader = StringReader::new("{color:'#ff0000',bold:true,unknown:1}");
        assert!(StyleArgumentType.parse(&mut reader).is_ok());
    }
}
