use std::sync::Arc;

use pumpkin_data::translation;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::loot_table::DynamicLootTable;
use pumpkin_util::loot_table::parser::{parse_condition_with_predicates, parse_functions};
use pumpkin_util::text::TextComponent;
use serde_json::Value;

use crate::command::CommandSource;
use crate::command::argument_types::FromStringReader;
use crate::command::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::snbt::SnbtParser;
use crate::command::string_reader::StringReader;

const ERROR_FAILED_TO_PARSE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_OR_ID_FAILED_TO_PARSE,
    translation::java::ARGUMENT_RESOURCE_OR_ID_FAILED_TO_PARSE,
);

const ERROR_NO_SUCH_ELEMENT: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_RESOURCE_OR_ID_NO_SUCH_ELEMENT,
    translation::java::ARGUMENT_RESOURCE_OR_ID_NO_SUCH_ELEMENT,
);

/// The loot registries a [`LootResourceArgumentType`] reads from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LootRegistry {
    Table,
    Modifier,
    Predicate,
}

impl LootRegistry {
    const fn name(self) -> &'static str {
        match self {
            Self::Table => "loot table",
            Self::Modifier => "item modifier",
            Self::Predicate => "predicate",
        }
    }

    const fn registry(self) -> &'static str {
        match self {
            Self::Table => "minecraft:loot_table",
            Self::Modifier => "minecraft:item_modifier",
            Self::Predicate => "minecraft:predicate",
        }
    }
}

/// A parsed `ResourceOrIdArgument`: a registry id, or an element written inline as SNBT.
#[derive(Clone)]
pub enum LootResource {
    Id(Identifier),
    /// An inline item modifier or predicate, in the JSON form the datapack loaders read.
    Inline(Arc<Value>),
    InlineTable(Arc<DynamicLootTable>),
}

/// Vanilla `ResourceOrIdArgument.lootTable`, `lootModifier` and `lootPredicate`.
pub struct LootResourceArgumentType(pub LootRegistry);

impl LootResourceArgumentType {
    pub fn get<'a>(
        context: &'a CommandContext,
        name: &str,
    ) -> Result<&'a LootResource, CommandSyntaxError> {
        context.get_argument(name)
    }

    /// Checks an inline element the way the registry's codec would decode it.
    fn decode(&self, value: Value) -> Option<LootResource> {
        let resolve = |name: &str| pumpkin_data::loot_table::get_loot_predicate(name).cloned();
        match self.0 {
            LootRegistry::Table => {
                let table =
                    crate::data::datapack::loot_table_loader::parse_loot_table(&value.to_string())?;
                Some(LootResource::InlineTable(Arc::new(table)))
            }
            // `LootItemFunctions.DIRECT_CODEC`: one function, or a list run as a sequence.
            LootRegistry::Modifier => {
                let written = value.as_array().map_or(1, Vec::len);
                ((value.is_object() || value.is_array())
                    && parse_functions(&value).len() == written)
                    .then(|| LootResource::Inline(Arc::new(value)))
            }
            LootRegistry::Predicate => parse_condition_with_predicates(&value, &resolve)
                .map(|_| LootResource::Inline(Arc::new(value))),
        }
    }
}

impl ArgumentType<CommandSource> for LootResourceArgumentType {
    type Item = LootResource;

    fn parse(&self, reader: &mut StringReader) -> Result<LootResource, CommandSyntaxError> {
        // `Term.alternative(id, value)`: an identifier wins whenever one can be read.
        if reader.peek().is_some_and(Identifier::is_valid_char) {
            return Ok(LootResource::Id(Identifier::from_reader(reader)?));
        }
        let tag = SnbtParser::parse_for_commands(reader)?;
        // SNBT has no booleans; `true` is a byte, which the JSON-reading loaders want as a bool.
        let value = pumpkin_protocol::ser::nbt_tag_to_json_ext(&tag, true);
        self.decode(value).ok_or_else(|| {
            ERROR_FAILED_TO_PARSE.create(
                reader,
                TextComponent::text(format!("Not a valid {}", self.0.name())),
            )
        })
    }

    fn parse_with_source(
        &self,
        reader: &mut StringReader,
        source: &CommandSource,
    ) -> Result<LootResource, CommandSyntaxError> {
        let resource = self.parse(reader)?;
        // Vanilla `ReferenceResult.parse`: an id has to name an element of the registry.
        if let LootResource::Id(id) = &resource
            && let Some(server) = &source.server
        {
            let name = id.to_string();
            let packs = &server.datapack_manager;
            let found = match self.0 {
                LootRegistry::Table => packs.get_loot_table(&name).is_some(),
                LootRegistry::Modifier => packs.get_item_modifier(&name).is_some(),
                LootRegistry::Predicate => packs.get_predicate(&name).is_some(),
            };
            if !found {
                return Err(ERROR_NO_SUCH_ELEMENT.create(
                    reader,
                    TextComponent::text(name),
                    TextComponent::text(self.0.registry()),
                ));
            }
        }
        Ok(resource)
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        match self.0 {
            LootRegistry::Table => JavaClientArgumentType::LootTable,
            LootRegistry::Modifier => JavaClientArgumentType::LootModifier,
            LootRegistry::Predicate => JavaClientArgumentType::LootPredicate,
        }
    }

    fn examples(&self) -> Vec<String> {
        crate::examples!("foo", "foo:bar", "012", "{}", "true")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(registry: LootRegistry, input: &str) -> Result<LootResource, CommandSyntaxError> {
        LootResourceArgumentType(registry).parse(&mut StringReader::new(input))
    }

    #[test]
    fn reads_ids_and_inline_elements() {
        assert!(matches!(
            parse(LootRegistry::Modifier, "minecraft:foo"),
            Ok(LootResource::Id(_))
        ));
        assert!(matches!(
            parse(
                LootRegistry::Modifier,
                r##"{type:"minecraft:exploration_map",destination:"#minecraft:on_treasure_maps"}"##
            ),
            Ok(LootResource::Inline(_))
        ));
        assert!(matches!(
            parse(
                LootRegistry::Modifier,
                r#"[{type:"minecraft:set_count",count:2},{type:"minecraft:set_count",count:1,add:true}]"#
            ),
            Ok(LootResource::Inline(_))
        ));
        assert!(matches!(
            parse(
                LootRegistry::Predicate,
                r#"{type:"minecraft:random_chance",chance:0.5f}"#
            ),
            Ok(LootResource::Inline(_))
        ));
        assert!(matches!(
            parse(
                LootRegistry::Table,
                r#"{pools:[{rolls:1,entries:[{type:"minecraft:item",name:"minecraft:stone"}]}]}"#
            ),
            Ok(LootResource::InlineTable(_))
        ));
        assert!(
            parse(
                LootRegistry::Modifier,
                r#"{type:"minecraft:no_such_function"}"#
            )
            .is_err()
        );
        assert!(
            parse(
                LootRegistry::Predicate,
                r#"{type:"minecraft:no_such_condition"}"#
            )
            .is_err()
        );
    }
}
