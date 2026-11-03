use pumpkin_data::{Block, BlockStateId, Property, translation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::argument_types::block::BlockArgumentType;
use crate::argument_types::block_predicate::{
    ERROR_NO_VALUE, ERROR_UNCLOSED_PROPERTIES, parse_nbt,
};
use crate::context::command_context::CommandContext;
use crate::errors::command_syntax_error::CommandSyntaxError;
use crate::errors::error_types::CommandErrorType;
use crate::string_reader::StringReader;
use crate::suggestion::suggestions::{Suggestions, SuggestionsBuilder};

pub const ERROR_UNKNOWN_PROPERTY: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNKNOWN,
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNKNOWN,
);

pub const ERROR_DUPLICATE_PROPERTY: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_DUPLICATE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_DUPLICATE,
);

pub const ERROR_INVALID_VALUE: CommandErrorType<3> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_INVALID,
    translation::java::ARGUMENT_BLOCK_PROPERTY_INVALID,
);

/// A parsed block state argument, like vanilla's `BlockInput`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockInput {
    pub state: BlockStateId,
    /// The properties written out in the argument, which placement keeps over shape updates.
    pub properties: Vec<&'static str>,
    /// Block entity data from a trailing `{...}`.
    pub nbt: Option<NbtCompound>,
}

/// A block with optional `[property=value,...]` state and `{...}` block entity data, like
/// vanilla's `BlockStateArgument`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockStateArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockStateArgumentType {
    type Item = BlockInput;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let block: &'static Block =
            ArgumentType::<crate::source::DummySource>::parse(&BlockArgumentType, reader)?;
        let (state, properties) = if reader.peek() == Some('[') {
            read_properties(block, reader)?
        } else {
            (block.default_state.id, Vec::new())
        };
        let nbt = parse_nbt(reader)?;
        Ok(BlockInput {
            state,
            properties,
            nbt,
        })
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::BlockState
    }

    fn examples(&self) -> Vec<String> {
        examples!("stone", "minecraft:stone", "stone[foo=bar]")
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder.build()
    }
}

/// Mirrors vanilla `BlockStateParser.readProperties`, applying each property to the default state.
/// Also returns the names of the given properties.
fn read_properties(
    block: &'static Block,
    reader: &mut StringReader,
) -> Result<(BlockStateId, Vec<&'static str>), CommandSyntaxError> {
    let block_name = || TextComponent::text(format!("minecraft:{}", block.name));
    let valid_properties: Vec<Property> = block
        .states
        .iter()
        .filter_map(|state| block.properties(state.id))
        .flat_map(|props| props.to_props())
        .collect();
    let mut properties = block
        .properties(block.default_state.id)
        .map(|props| props.to_props())
        .unwrap_or_default();
    let mut seen: Vec<&'static str> = Vec::new();

    reader.skip();
    reader.skip_whitespace();
    while reader.can_read_char() && reader.peek() != Some(']') {
        reader.skip_whitespace();
        let key_start = reader.cursor();
        let key = reader.read_string()?;
        let Some(name) = valid_properties
            .iter()
            .map(|p| p.key().to_str())
            .find(|name| *name == key)
        else {
            reader.set_cursor(key_start);
            return Err(ERROR_UNKNOWN_PROPERTY.create(
                reader,
                block_name(),
                TextComponent::text(key),
            ));
        };
        if seen.contains(&name) {
            reader.set_cursor(key_start);
            return Err(ERROR_DUPLICATE_PROPERTY.create(
                reader,
                TextComponent::text(key),
                block_name(),
            ));
        }
        seen.push(name);

        reader.skip_whitespace();
        if reader.peek() != Some('=') {
            return Err(ERROR_NO_VALUE.create(reader, TextComponent::text(key)));
        }
        reader.skip();
        reader.skip_whitespace();

        let value_start = reader.cursor();
        let raw = reader.read_string()?;
        let Some(&value) = valid_properties
            .iter()
            .find(|p| p.key().to_str() == name && p.value_str() == raw)
        else {
            reader.set_cursor(value_start);
            return Err(ERROR_INVALID_VALUE.create(
                reader,
                block_name(),
                TextComponent::text(raw),
                TextComponent::text(name),
            ));
        };
        if let Some(entry) = properties.iter_mut().find(|p| p.key().to_str() == name) {
            *entry = value;
        }

        reader.skip_whitespace();
        if reader.can_read_char() {
            if reader.peek() != Some(',') {
                if reader.peek() != Some(']') {
                    return Err(ERROR_UNCLOSED_PROPERTIES.create(reader));
                }
                break;
            }
            reader.skip();
        }
    }

    if reader.can_read_char() {
        reader.skip();
    } else {
        return Err(ERROR_UNCLOSED_PROPERTIES.create(reader));
    }

    Ok((block.from_properties(&properties).to_state_id(block), seen))
}

impl BlockStateArgumentType {
    /// Returns a `CommandContext`'s parsed [`BlockInput`] argument.
    pub fn get<'a, S: crate::source::CommandSource>(
        context: &'a CommandContext<S>,
        name: &str,
    ) -> Result<&'a BlockInput, CommandSyntaxError> {
        context.get_argument(name)
    }
}

#[cfg(test)]
mod test {
    use pumpkin_data::BlockStateId;

    use crate::argument_types::argument_type::ArgumentType;
    use crate::argument_types::block_predicate::ERROR_UNCLOSED_PROPERTIES;
    use crate::argument_types::block_state::{
        BlockInput, BlockStateArgumentType, ERROR_DUPLICATE_PROPERTY, ERROR_INVALID_VALUE,
        ERROR_UNKNOWN_PROPERTY,
    };
    use crate::string_reader::StringReader;

    fn parse(input: &str) -> BlockInput {
        let mut reader = StringReader::new(input);
        let parsed =
            ArgumentType::<crate::source::DummySource>::parse(&BlockStateArgumentType, &mut reader)
                .unwrap();
        assert!(!reader.can_read_char(), "{input} was not read to the end");
        parsed
    }

    // State ids are taken from the extracted vanilla `assets/blocks.json`.
    #[test]
    fn parse_properties() {
        let input = parse("birch_stairs");
        assert_eq!(input.state, BlockStateId::new(11561).unwrap());
        assert!(input.properties.is_empty());

        let input = parse("minecraft:birch_stairs[facing=west]");
        assert_eq!(input.state, BlockStateId::new(11601).unwrap());
        assert_eq!(input.properties, ["facing"]);

        let input = parse("birch_stairs[ half = top , facing=west ]");
        assert_eq!(input.state, BlockStateId::new(11591).unwrap());
        assert_eq!(input.properties, ["half", "facing"]);
    }

    #[test]
    fn parse_block_entity_data() {
        let input = parse(r#"chest[facing=south]{Items:[{Slot:0b,id:"minecraft:stone",count:1}]}"#);
        let items = input.nbt.as_ref().and_then(|nbt| nbt.get_list("Items"));
        assert_eq!(items.map(<[_]>::len), Some(1));
        assert!(parse("barrel{}").nbt.is_some_and(|nbt| nbt.is_empty()));
        assert_eq!(parse("barrel").nbt, None);
    }

    #[test]
    fn reject_bad_properties() {
        let mut reader = StringReader::new("birch_stairs[color=red]");
        assert_parse_err_reset!(&mut reader, BlockStateArgumentType, &ERROR_UNKNOWN_PROPERTY);

        let mut reader = StringReader::new("birch_stairs[facing=up]");
        assert_parse_err_reset!(&mut reader, BlockStateArgumentType, &ERROR_INVALID_VALUE);

        let mut reader = StringReader::new("birch_stairs[facing=west,facing=east]");
        assert_parse_err_reset!(
            &mut reader,
            BlockStateArgumentType,
            &ERROR_DUPLICATE_PROPERTY
        );

        let mut reader = StringReader::new("birch_stairs[facing=west");
        assert_parse_err_reset!(
            &mut reader,
            BlockStateArgumentType,
            &ERROR_UNCLOSED_PROPERTIES
        );
    }
}
