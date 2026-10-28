use std::collections::HashMap;

use pumpkin_data::tag::{RegistryKey, get_tag_ids};
use pumpkin_data::{Block, BlockStateId, translation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::argument_types::block::INVALID_BLOCK_ERROR_TYPE;
use crate::argument_types::nbt::NbtCompoundArgumentType;
use crate::argument_types::nbt_path::compare_nbt;
use crate::context::command_context::CommandContext;
use crate::errors::command_syntax_error::CommandSyntaxError;
use crate::errors::error_types::CommandErrorType;
use crate::string_reader::StringReader;
use crate::suggestion::suggestions::{Suggestions, SuggestionsBuilder};

pub const ERROR_UNKNOWN_TAG: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_BLOCK_TAG_UNKNOWN,
    translation::java::ARGUMENTS_BLOCK_TAG_UNKNOWN,
);

pub const ERROR_NO_VALUE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
);

pub const ERROR_UNCLOSED_PROPERTIES: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNCLOSED,
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNCLOSED,
);

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

#[derive(Clone, Debug)]
pub enum BlockPredicate {
    Block {
        block: &'static Block,
        properties: HashMap<String, String>,
        nbt: Option<NbtCompound>,
    },
    Tag {
        tag_name: String,
        block_ids: Vec<u16>,
        properties: HashMap<String, String>,
        nbt: Option<NbtCompound>,
    },
}

impl BlockPredicate {
    /// Tests a block like vanilla's `BlockPredicateArgument` results: every listed property has to
    /// match, and NBT is a partial match against the block entity's full save data.
    /// `block_entity_nbt` is only called when the predicate has NBT.
    pub fn test(
        &self,
        block: &Block,
        state_id: BlockStateId,
        block_entity_nbt: impl FnOnce() -> Option<NbtCompound>,
    ) -> bool {
        let (matches_block, properties, nbt) = match self {
            Self::Block {
                block: expected,
                properties,
                nbt,
            } => (block.id == expected.id, properties, nbt),
            Self::Tag {
                block_ids,
                properties,
                nbt,
                ..
            } => (block_ids.contains(&block.id.as_u16()), properties, nbt),
        };
        if !matches_block {
            return false;
        }

        // A tag's properties aren't checked at parse time, so a block without the property or
        // with a value it can't take simply doesn't match.
        if !properties.is_empty() {
            let actual = block
                .properties(state_id)
                .map(|props| props.to_props())
                .unwrap_or_default();
            let all_match = properties.iter().all(|(key, value)| {
                actual
                    .iter()
                    .any(|prop| prop.key_str() == key && prop.value_str() == value)
            });
            if !all_match {
                return false;
            }
        }

        let Some(expected) = nbt else {
            return true;
        };
        block_entity_nbt().is_some_and(|actual| {
            expected.child_tags.iter().all(|(key, expected_value)| {
                actual
                    .child_tags
                    .get(key)
                    .is_some_and(|actual_value| compare_nbt(expected_value, actual_value))
            })
        })
    }

    #[must_use]
    pub const fn requires_nbt(&self) -> bool {
        match self {
            Self::Block { nbt, .. } | Self::Tag { nbt, .. } => nbt.is_some(),
        }
    }
}

/// Reads `[key=value,...]`. For a concrete `block`, unknown properties and values the block can't
/// take are errors like in vanilla's `BlockStateParser`; tag properties are only checked when
/// tested.
fn parse_properties(
    reader: &mut StringReader,
    id: &str,
    block: Option<&'static Block>,
) -> Result<HashMap<String, String>, CommandSyntaxError> {
    let mut properties = HashMap::new();
    if reader.peek() == Some('[') {
        reader.skip();
        reader.skip_whitespace();
        while reader.can_read_char() && reader.peek() != Some(']') {
            let start_key = reader.cursor();
            while let Some(c) = reader.peek() {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    reader.skip();
                } else {
                    break;
                }
            }
            let key = reader.string()[start_key..reader.cursor()].to_string();
            if let Some(block) = block
                && !has_property(block, &key)
            {
                reader.set_cursor(start_key);
                return Err(ERROR_UNKNOWN_PROPERTY.create(
                    reader,
                    TextComponent::text(id.to_string()),
                    TextComponent::text(key),
                ));
            }
            if properties.contains_key(&key) {
                reader.set_cursor(start_key);
                return Err(ERROR_DUPLICATE_PROPERTY.create(
                    reader,
                    TextComponent::text(id.to_string()),
                    TextComponent::text(key),
                ));
            }
            reader.skip_whitespace();
            if reader.peek() != Some('=') {
                return Err(ERROR_NO_VALUE.create(
                    reader,
                    TextComponent::text(key),
                    TextComponent::text(id.to_string()),
                ));
            }
            reader.skip();
            reader.skip_whitespace();
            let start_val = reader.cursor();
            while let Some(c) = reader.peek() {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    reader.skip();
                } else {
                    break;
                }
            }
            let val = reader.string()[start_val..reader.cursor()].to_string();
            if let Some(block) = block
                && !has_property_value(block, &key, &val)
            {
                reader.set_cursor(start_val);
                return Err(ERROR_INVALID_VALUE.create(
                    reader,
                    TextComponent::text(id.to_string()),
                    TextComponent::text(val),
                    TextComponent::text(key),
                ));
            }
            properties.insert(key, val);
            reader.skip_whitespace();
            if reader.peek() == Some(',') {
                reader.skip();
                reader.skip_whitespace();
            } else if reader.peek() == Some(']') {
                break;
            } else {
                return Err(ERROR_UNCLOSED_PROPERTIES.create_without_context());
            }
        }
        if reader.peek() == Some(']') {
            reader.skip();
        } else {
            return Err(ERROR_UNCLOSED_PROPERTIES.create_without_context());
        }
    }
    Ok(properties)
}

fn has_property(block: &Block, key: &str) -> bool {
    block
        .properties(block.default_state.id)
        .is_some_and(|props| props.to_props().iter().any(|prop| prop.key_str() == key))
}

fn has_property_value(block: &Block, key: &str, value: &str) -> bool {
    block.states.iter().any(|state| {
        block.properties(state.id).is_some_and(|props| {
            props
                .to_props()
                .iter()
                .any(|prop| prop.key_str() == key && prop.value_str() == value)
        })
    })
}

fn parse_nbt(reader: &mut StringReader) -> Result<Option<NbtCompound>, CommandSyntaxError> {
    if reader.peek() == Some('{') {
        let tag =
            ArgumentType::<crate::source::DummySource>::parse(&NbtCompoundArgumentType, reader)?;
        Ok(Some(tag))
    } else {
        Ok(None)
    }
}

pub struct BlockPredicateArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockPredicateArgumentType {
    type Item = BlockPredicate;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        if reader.peek() == Some('#') {
            reader.skip();
            let start = reader.cursor();
            while let Some(c) = reader.peek() {
                if c.is_alphanumeric() || c == '_' || c == ':' || c == '/' || c == '.' || c == '-' {
                    reader.skip();
                } else {
                    break;
                }
            }
            let tag_str = &reader.string()[start..reader.cursor()];
            let normalized = if tag_str.contains(':') {
                tag_str.to_string()
            } else {
                format!("minecraft:{tag_str}")
            };
            let stripped = normalized.strip_prefix("minecraft:").unwrap_or(&normalized);

            let tag_ids = get_tag_ids(RegistryKey::Block, &normalized)
                .or_else(|| get_tag_ids(RegistryKey::Block, stripped));

            let block_ids = match tag_ids {
                Some(ids) => ids.to_vec(),
                None => {
                    return Err(ERROR_UNKNOWN_TAG.create(reader, TextComponent::text(normalized)));
                }
            };

            let properties = parse_properties(reader, &normalized, None)?;
            let nbt = parse_nbt(reader)?;

            Ok(BlockPredicate::Tag {
                tag_name: normalized,
                block_ids,
                properties,
                nbt,
            })
        } else {
            let start = reader.cursor();
            while let Some(c) = reader.peek() {
                if c.is_alphanumeric() || c == '_' || c == ':' || c == '/' || c == '.' || c == '-' {
                    reader.skip();
                } else {
                    break;
                }
            }
            let block_str = &reader.string()[start..reader.cursor()];
            let normalized = if block_str.contains(':') {
                block_str.to_string()
            } else {
                format!("minecraft:{block_str}")
            };

            let block = Block::from_name(&normalized).ok_or_else(|| {
                INVALID_BLOCK_ERROR_TYPE.create(reader, TextComponent::text(normalized.clone()))
            })?;

            let properties = parse_properties(reader, &normalized, Some(block))?;
            let nbt = parse_nbt(reader)?;

            Ok(BlockPredicate::Block {
                block,
                properties,
                nbt,
            })
        }
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::BlockPredicate
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "stone".to_string(),
            "minecraft:stone".to_string(),
            "stone[foo=bar]".to_string(),
            "#stone".to_string(),
            "#stone[foo=bar]{baz=nbt}".to_string(),
        ]
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder.build()
    }
}

impl BlockPredicateArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<BlockPredicate, CommandSyntaxError> {
        Ok(context.get_argument::<BlockPredicate>(name)?.clone())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::source::DummySource;

    fn parse(input: &str) -> Result<BlockPredicate, CommandSyntaxError> {
        ArgumentType::<DummySource>::parse(
            &BlockPredicateArgumentType,
            &mut StringReader::new(input),
        )
    }

    fn state_with(block: &Block, key: &str, value: &str) -> BlockStateId {
        block
            .states
            .iter()
            .map(|state| state.id)
            .find(|&id| {
                block.properties(id).is_some_and(|props| {
                    props
                        .to_props()
                        .iter()
                        .any(|prop| prop.key_str() == key && prop.value_str() == value)
                })
            })
            .unwrap()
    }

    #[test]
    fn block_properties_are_checked_while_parsing() {
        assert!(parse("repeater[delay=1]").is_ok());
        assert!(parse("repeater[charge=1]").is_err());
        assert!(parse("repeater[delay=5]").is_err());
        assert!(parse("repeater[delay=1,delay=2]").is_err());
        // Tag properties are only resolved per block when testing.
        assert!(parse("#minecraft:doors[charge=1]").is_ok());
    }

    #[test]
    fn only_listed_properties_have_to_match() {
        let delay_3 = state_with(&Block::REPEATER, "delay", "3");
        let none = || None;
        assert!(
            parse("repeater")
                .unwrap()
                .test(&Block::REPEATER, delay_3, none)
        );
        assert!(
            parse("repeater[delay=3]")
                .unwrap()
                .test(&Block::REPEATER, delay_3, none)
        );
        assert!(
            !parse("repeater[delay=1]")
                .unwrap()
                .test(&Block::REPEATER, delay_3, none)
        );
    }

    #[test]
    fn tag_properties_the_block_lacks_do_not_match() {
        let open = state_with(&Block::OAK_DOOR, "open", "true");
        let none = || None;
        assert!(
            parse("#minecraft:doors[open=true]")
                .unwrap()
                .test(&Block::OAK_DOOR, open, none)
        );
        assert!(
            !parse("#minecraft:doors[open=false]")
                .unwrap()
                .test(&Block::OAK_DOOR, open, none)
        );
        assert!(
            !parse("#minecraft:doors[delay=1]")
                .unwrap()
                .test(&Block::OAK_DOOR, open, none)
        );
    }

    #[test]
    fn nbt_is_a_partial_match_against_the_block_entity() {
        let predicate = parse("chest{CustomName:\"loot\"}").unwrap();
        let state = Block::CHEST.default_state.id;
        let saved = |name: &str| {
            let mut nbt = NbtCompound::new();
            nbt.put_string("id", "minecraft:chest".to_string());
            nbt.put_int("x", 0);
            nbt.put_string("CustomName", name.to_string());
            Some(nbt)
        };
        assert!(predicate.test(&Block::CHEST, state, || saved("loot")));
        assert!(!predicate.test(&Block::CHEST, state, || saved("other")));
        assert!(!predicate.test(&Block::CHEST, state, || None));
    }
}
