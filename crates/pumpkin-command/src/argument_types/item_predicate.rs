use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{CustomDataImpl, DamageImpl, MaxDamageImpl, read_data};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{RegistryKey, get_tag_ids};
use pumpkin_data::translation;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::TextComponent;

use crate::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::argument_types::item::{
    ERROR_EXPECTED_COMPONENT, ERROR_MALFORMED_COMPONENT, ERROR_UNKNOWN_COMPONENT,
};
use crate::argument_types::nbt_path::compare_nbt;
use crate::context::command_context::CommandContext;
use crate::errors::command_syntax_error::CommandSyntaxError;
use crate::errors::error_types::CommandErrorType;
use crate::snbt::SnbtParser;
use crate::string_reader::StringReader;
use crate::suggestion::suggestions::{Suggestions, SuggestionsBuilder};

pub const ERROR_UNKNOWN_ITEM: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_ITEM_ID_INVALID,
    translation::java::ARGUMENT_ITEM_ID_INVALID,
);

pub const ERROR_UNKNOWN_TAG: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_ITEM_TAG_UNKNOWN,
    translation::java::ARGUMENTS_ITEM_TAG_UNKNOWN,
);

pub const ERROR_UNKNOWN_PREDICATE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_ITEM_PREDICATE_UNKNOWN,
    translation::java::ARGUMENTS_ITEM_PREDICATE_UNKNOWN,
);

/// Vanilla `ItemPredicateArgument.COUNT_ID`, the pseudo component and predicate for the count.
const COUNT: &str = "count";

/// The `DataComponentPredicate` types vanilla registers, besides component existence.
const PREDICATE_TYPES: [&str; 15] = [
    "damage",
    "enchantments",
    "stored_enchantments",
    "potion_contents",
    "custom_data",
    "container",
    "bundle_contents",
    "firework_explosion",
    "fireworks",
    "writable_book_content",
    "written_book_content",
    "attribute_modifiers",
    "trim",
    "jukebox_playable",
    "villager/variant",
];

#[derive(Clone)]
pub enum ItemPredicate {
    Item(&'static Item),
    Tag(Vec<u16>),
    Any,
    /// A base predicate and its `[...]` tests: every list must have a passing alternative.
    Filtered(Box<Self>, Vec<Vec<ComponentTest>>),
}

/// One test of vanilla's `ComponentPredicateParser` grammar.
#[derive(Clone)]
pub struct ComponentTest {
    negated: bool,
    kind: ComponentTestKind,
}

#[derive(Clone)]
enum ComponentTestKind {
    /// `component`: the stack has the component, defaults included.
    Exists(DataComponent),
    /// `component=value`: the component equals the value.
    Equals(DataComponent, NbtTag),
    /// `count`, `count=range` or `count~range`.
    Count(Option<NbtTag>),
    /// `predicate~value`, a `DataComponentPredicate`.
    Predicate(&'static str, NbtTag),
}

/// Evaluates the `DataComponentPredicate`s this crate doesn't know, given their type and value.
pub type SubPredicate<'a> = &'a dyn Fn(&ItemStack, &str, &NbtTag) -> bool;

impl ItemPredicate {
    /// Tests a stack, treating `DataComponentPredicate`s other than `custom_data`, `damage`
    /// and `count` as failing; use [`Self::test_with`] to evaluate them.
    #[must_use]
    pub fn test(&self, stack: &ItemStack) -> bool {
        self.test_with(stack, &|_, _, _| false)
    }

    #[must_use]
    pub fn test_with(&self, stack: &ItemStack, sub_predicate: SubPredicate) -> bool {
        match self {
            Self::Any => true,
            Self::Item(item) => stack.get_item().id == item.id,
            Self::Tag(tag) => tag.contains(&stack.get_item().id),
            Self::Filtered(base, tests) => {
                base.test_with(stack, sub_predicate)
                    && tests.iter().all(|alternatives| {
                        alternatives
                            .iter()
                            .any(|test| test.test(stack, sub_predicate) != test.negated)
                    })
            }
        }
    }
}

pub struct ItemPredicateArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for ItemPredicateArgumentType {
    type Item = ItemPredicate;

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

            let tag_ids = get_tag_ids(RegistryKey::Item, &normalized)
                .or_else(|| get_tag_ids(RegistryKey::Item, stripped));

            let items = match tag_ids {
                Some(ids) => ids.to_vec(),
                None => {
                    return Err(ERROR_UNKNOWN_TAG.create(reader, TextComponent::text(normalized)));
                }
            };

            with_components(ItemPredicate::Tag(items), reader)
        } else {
            let start = reader.cursor();
            while let Some(c) = reader.peek() {
                if c.is_alphanumeric()
                    || c == '_'
                    || c == ':'
                    || c == '/'
                    || c == '.'
                    || c == '-'
                    || c == '*'
                {
                    reader.skip();
                } else {
                    break;
                }
            }
            let item_str = &reader.string()[start..reader.cursor()];
            if item_str == "*" {
                return with_components(ItemPredicate::Any, reader);
            }

            let normalized = if item_str.contains(':') {
                item_str.to_string()
            } else {
                format!("minecraft:{item_str}")
            };
            let stripped = normalized.strip_prefix("minecraft:").unwrap_or(&normalized);

            let item = Item::from_registry_key(&normalized)
                .or_else(|| Item::from_registry_key(stripped))
                .ok_or_else(|| {
                    ERROR_UNKNOWN_ITEM.create(reader, TextComponent::text(normalized))
                })?;

            with_components(ItemPredicate::Item(item), reader)
        }
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::ItemPredicate
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "stick".to_string(),
            "minecraft:stick".to_string(),
            "#stick".to_string(),
            "#stick{foo:'bar'}".to_string(),
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

impl ComponentTest {
    fn test(&self, stack: &ItemStack, sub_predicate: SubPredicate) -> bool {
        match &self.kind {
            ComponentTestKind::Exists(component) => stack.get_component(*component).is_some(),
            ComponentTestKind::Equals(component, value) => {
                match (
                    stack.get_component(*component),
                    read_data(*component, value),
                ) {
                    (Some(actual), Some(expected)) => actual.equal(expected.as_ref()),
                    _ => false,
                }
            }
            ComponentTestKind::Count(range) => range
                .as_ref()
                .is_none_or(|range| int_range_matches(range, i32::from(stack.item_count))),
            ComponentTestKind::Predicate(name, value) => match *name {
                // `CustomDataPredicate`: `NbtPredicate`, also given as an SNBT string.
                "custom_data" => {
                    let expected = match value {
                        NbtTag::String(snbt) => {
                            let mut reader = StringReader::new(snbt.as_ref());
                            match SnbtParser::parse_for_commands(&mut reader) {
                                Ok(tag) => tag,
                                Err(_) => return false,
                            }
                        }
                        tag => tag.clone(),
                    };
                    let actual = NbtTag::Compound(
                        stack
                            .get_data_component::<CustomDataImpl>()
                            .map(|data| data.data.clone())
                            .unwrap_or_default(),
                    );
                    compare_nbt(&expected, &actual)
                }
                // `DamagePredicate`: needs a damageable stack.
                "damage" => {
                    let (Some(damage), Some(max)) = (
                        stack.get_data_component::<DamageImpl>(),
                        stack.get_data_component::<MaxDamageImpl>(),
                    ) else {
                        return false;
                    };
                    let NbtTag::Compound(value) = value else {
                        return false;
                    };
                    value.get("durability").is_none_or(|range| {
                        int_range_matches(range, max.max_damage - damage.damage)
                    }) && value
                        .get("damage")
                        .is_none_or(|range| int_range_matches(range, damage.damage))
                }
                name => sub_predicate(stack, name, value),
            },
        }
    }
}

/// `MinMaxBounds.Ints.CODEC`: an exact number or `{min, max}`, both optional.
fn int_range_matches(range: &NbtTag, value: i32) -> bool {
    if let Some(exact) = range.as_numeric_int() {
        return value == exact;
    }
    let NbtTag::Compound(range) = range else {
        return false;
    };
    range
        .get("min")
        .and_then(NbtTag::as_numeric_int)
        .is_none_or(|min| value >= min)
        && range
            .get("max")
            .and_then(NbtTag::as_numeric_int)
            .is_none_or(|max| value <= max)
}

/// Reads the optional `[...]` after the item, tag or `*` (vanilla `ComponentPredicateParser`):
/// `,` separates tests that must all pass, `|` alternatives, `!` negates a test.
fn with_components(
    base: ItemPredicate,
    reader: &mut StringReader,
) -> Result<ItemPredicate, CommandSyntaxError> {
    if reader.peek() != Some('[') {
        skip_nbt(reader);
        return Ok(base);
    }
    reader.skip();
    let mut tests = Vec::new();
    reader.skip_whitespace();
    if reader.peek() == Some(']') {
        reader.skip();
        return Ok(base);
    }
    loop {
        let mut alternatives = vec![read_test(reader)?];
        reader.skip_whitespace();
        while reader.peek() == Some('|') {
            reader.skip();
            alternatives.push(read_test(reader)?);
            reader.skip_whitespace();
        }
        tests.push(alternatives);
        match reader.peek() {
            Some(',') => reader.skip(),
            Some(']') => {
                reader.skip();
                break;
            }
            _ => return Err(ERROR_EXPECTED_COMPONENT.create(reader)),
        }
    }
    skip_nbt(reader);
    Ok(ItemPredicate::Filtered(Box::new(base), tests))
}

fn read_test(reader: &mut StringReader) -> Result<ComponentTest, CommandSyntaxError> {
    reader.skip_whitespace();
    let negated = reader.peek() == Some('!');
    if negated {
        reader.skip();
    }
    let start = reader.cursor();
    let id = <pumpkin_util::identifier::Identifier as crate::argument_types::FromStringReader>::from_reader(reader)?
        .to_string();
    let name = id.strip_prefix("minecraft:").unwrap_or(&id).to_string();
    reader.skip_whitespace();
    let kind = match reader.peek() {
        Some('=') => {
            reader.skip();
            reader.skip_whitespace();
            let value = SnbtParser::parse_for_commands(reader)?;
            if name == COUNT {
                ComponentTestKind::Count(Some(value))
            } else {
                let component = component_or_error(reader, start, &id)?;
                if read_data(component, &value).is_none() {
                    reader.set_cursor(start);
                    return Err(ERROR_MALFORMED_COMPONENT.create(
                        reader,
                        TextComponent::text(id),
                        TextComponent::text(value.to_string()),
                    ));
                }
                ComponentTestKind::Equals(component, value)
            }
        }
        Some('~') => {
            reader.skip();
            reader.skip_whitespace();
            let value = SnbtParser::parse_for_commands(reader)?;
            if name == COUNT {
                ComponentTestKind::Count(Some(value))
            } else if let Some(&predicate) = PREDICATE_TYPES.iter().find(|&&p| p == name) {
                ComponentTestKind::Predicate(predicate, value)
            } else if DataComponent::try_from_name(&name).is_some() {
                // A component without a predicate type only checks that it exists.
                ComponentTestKind::Exists(component_or_error(reader, start, &id)?)
            } else {
                reader.set_cursor(start);
                return Err(ERROR_UNKNOWN_PREDICATE.create(reader, TextComponent::text(id)));
            }
        }
        _ if name == COUNT => ComponentTestKind::Count(None),
        _ => ComponentTestKind::Exists(component_or_error(reader, start, &id)?),
    };
    Ok(ComponentTest { negated, kind })
}

fn component_or_error(
    reader: &mut StringReader,
    start: usize,
    id: &str,
) -> Result<DataComponent, CommandSyntaxError> {
    DataComponent::try_from_name(id).ok_or_else(|| {
        reader.set_cursor(start);
        ERROR_UNKNOWN_COMPONENT.create(reader, TextComponent::text(id.to_string()))
    })
}

/// Pre-1.20.5 predicates could end in `{nbt}`; it's skipped.
fn skip_nbt(reader: &mut StringReader) {
    if reader.peek() == Some('{') {
        reader.skip();
        let mut depth = 1;
        while reader.can_read_char() && depth > 0 {
            match reader.read() {
                Some('{') => depth += 1,
                Some('}') => depth -= 1,
                _ => {}
            }
        }
    }
}

impl ItemPredicateArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<ItemPredicate, CommandSyntaxError> {
        Ok(context.get_argument::<ItemPredicate>(name)?.clone())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::argument_types::item::ItemStackArgumentType;
    use crate::source::DummySource;

    fn predicate(input: &str) -> Result<ItemPredicate, CommandSyntaxError> {
        ArgumentType::<DummySource>::parse(
            &ItemPredicateArgumentType,
            &mut StringReader::new(input),
        )
    }

    fn stack(input: &str) -> ItemStack {
        ArgumentType::<DummySource>::parse(&ItemStackArgumentType, &mut StringReader::new(input))
            .unwrap()
    }

    #[test]
    fn component_filters_follow_the_vanilla_grammar() {
        let tagged = stack("stick[custom_data={car:1b,color:\"red\"}]");
        let plain = stack("stick");
        let sword = stack("diamond_sword[damage=5]");
        for (input, tagged_matches, plain_matches, sword_matches) in [
            ("*[custom_data~{car:1b}]", true, false, false),
            ("stick[custom_data={car:1b}]", false, false, false),
            ("stick[!custom_data]", false, true, false),
            ("*[damage~{damage:{min:1}}]", false, false, true),
            ("*[damage=0|custom_data]", true, false, false),
            ("*[count=1,max_stack_size=64]", true, true, false),
            ("*[damage]", false, false, true),
        ] {
            let predicate = predicate(input).unwrap();
            assert_eq!(
                predicate.test(&tagged),
                tagged_matches,
                "{input} on the tagged stick"
            );
            assert_eq!(
                predicate.test(&plain),
                plain_matches,
                "{input} on the plain stick"
            );
            assert_eq!(
                predicate.test(&sword),
                sword_matches,
                "{input} on the sword"
            );
        }
        assert!(predicate("*[not_a_component]").is_err());
        assert!(predicate("*[unbreakable~{}]").is_ok());
    }
}
