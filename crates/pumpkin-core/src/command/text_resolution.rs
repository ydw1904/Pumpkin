//! Vanilla `ComponentUtils.resolve`: fills in the selector, score and NBT parts of a text
//! component for a command source before it is sent.

use std::borrow::Cow;
use std::cell::Cell;
use std::sync::Arc;

use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::color::NamedColor;
use pumpkin_util::text::hover::HoverEvent;
use pumpkin_util::text::style::Style;
use pumpkin_util::text::{ScoreContent, TextComponent, TextComponentBase, TextContent};

use crate::command::CommandSource;
use crate::command::argument_types::argument_type::ArgumentType;
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::argument_types::entity;
use crate::command::argument_types::entity_selector::EntitySelector;
use crate::command::argument_types::entity_selector::parser::EntitySelectorParser;
use crate::command::argument_types::nbt_path::NbtPathArgumentType;
use crate::command::commands::data::{
    BlockDataAccessor, DataAccessor, EntityDataAccessor, StorageDataAccessor, snbt_colorful_display,
};
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::string_reader::StringReader;
use crate::entity::EntityBase;
use pumpkin_protocol::NumberFormat;

/// `ResolutionContext.Builder.depthLimit`.
const DEPTH_LIMIT: usize = 100;
/// `ResolutionContext.DEFAULT_RESOLVED_COMPONENT_LIMIT`: how many NBT components one resolution
/// may read.
const DEFAULT_RESOLVED_COMPONENT_LIMIT: usize = 65536;

/// Vanilla `ResolutionContext`.
struct ResolutionContext<'a> {
    source: &'a CommandSource,
    default_scoreboard_entity: Option<&'a Arc<dyn EntityBase>>,
    resolved_component_count: Cell<usize>,
}

/// Vanilla `ComponentUtils.resolve(ResolutionContext.create(source), component)`.
pub fn resolve(
    source: &CommandSource,
    component: &TextComponent,
) -> Result<TextComponent, CommandSyntaxError> {
    resolve_for_entity(source, component, source.entity.as_ref())
}

/// Vanilla `ComponentUtils.resolve` with `withEntityOverride(entity)`: `*` in a score component
/// means `entity`.
pub fn resolve_for_entity(
    source: &CommandSource,
    component: &TextComponent,
    entity: Option<&Arc<dyn EntityBase>>,
) -> Result<TextComponent, CommandSyntaxError> {
    let context = ResolutionContext {
        source,
        default_scoreboard_entity: entity,
        resolved_component_count: Cell::new(0),
    };
    Ok(TextComponent(context.resolve(&component.0, 0)?))
}

/// Vanilla `Style.applyTo`: the fields `style` sets, then those of `inner`.
fn apply_to(style: &Style, inner: &Style) -> Style {
    Style {
        color: style.color.or(inner.color),
        bold: style.bold.or(inner.bold),
        italic: style.italic.or(inner.italic),
        underlined: style.underlined.or(inner.underlined),
        strikethrough: style.strikethrough.or(inner.strikethrough),
        obfuscated: style.obfuscated.or(inner.obfuscated),
        insertion: style.insertion.clone().or_else(|| inner.insertion.clone()),
        click_event: style
            .click_event
            .clone()
            .or_else(|| inner.click_event.clone()),
        hover_event: style
            .hover_event
            .clone()
            .or_else(|| inner.hover_event.clone()),
        font: style.font.clone().or_else(|| inner.font.clone()),
        shadow_color: style.shadow_color.or(inner.shadow_color),
    }
}

fn plain(content: TextContent) -> TextComponentBase {
    TextComponent::from_content(content).0
}

fn empty() -> TextComponentBase {
    TextComponent::empty().0
}

/// Vanilla `ComponentUtils.formatList`.
fn format_list(
    mut values: Vec<TextComponentBase>,
    separator: &TextComponentBase,
) -> TextComponentBase {
    if values.len() == 1 {
        return values.remove(0);
    }
    let mut result = empty();
    for (i, value) in values.into_iter().enumerate() {
        if i > 0 {
            result.extra.push(separator.clone());
        }
        result.extra.push(value);
    }
    result
}

fn parse_selector(selector: &str) -> Result<EntitySelector, CommandSyntaxError> {
    let mut reader = StringReader::new(selector);
    EntitySelectorParser::new(&mut reader, true).parse_and_consume()
}

impl ResolutionContext<'_> {
    fn resolve(
        &self,
        component: &TextComponentBase,
        depth: usize,
    ) -> Result<TextComponentBase, CommandSyntaxError> {
        // `LimitBehavior.STOP_PROCESSING_AND_COPY_REMAINING`.
        if depth > DEPTH_LIMIT {
            return Ok(component.clone());
        }
        let mut result = self.resolve_contents(&component.content, depth + 1)?;
        for sibling in &component.extra {
            result.extra.push(self.resolve(sibling, depth + 1)?);
        }
        let style = self.resolve_style(&component.style, depth)?;
        // `MutableComponent.withStyle(Style)`.
        result.style = Box::new(apply_to(&style, &result.style));
        Ok(result)
    }

    fn resolve_style(&self, style: &Style, depth: usize) -> Result<Style, CommandSyntaxError> {
        let mut style = style.clone();
        if let Some(HoverEvent::ShowText { value }) = &style.hover_event {
            let resolved = value
                .iter()
                .map(|text| self.resolve(text, depth + 1))
                .collect::<Result<_, _>>()?;
            style.hover_event = Some(HoverEvent::ShowText { value: resolved });
        }
        Ok(style)
    }

    fn resolve_optional(
        &self,
        component: Option<&TextComponentBase>,
        depth: usize,
    ) -> Result<Option<TextComponentBase>, CommandSyntaxError> {
        component.map(|c| self.resolve(c, depth)).transpose()
    }

    /// Vanilla `ComponentContents.resolve`.
    fn resolve_contents(
        &self,
        content: &TextContent,
        depth: usize,
    ) -> Result<TextComponentBase, CommandSyntaxError> {
        match content {
            // `TranslatableContents.resolve`: only component arguments are resolved.
            TextContent::Translate {
                translate,
                bedrock_translate,
                with,
            } => Ok(plain(TextContent::Translate {
                translate: translate.clone(),
                bedrock_translate: bedrock_translate.clone(),
                with: with
                    .iter()
                    .map(|arg| self.resolve(arg, depth))
                    .collect::<Result<_, _>>()?,
            })),
            TextContent::EntityNames {
                selector,
                separator,
            } => self.resolve_selector(selector, separator.as_deref()),
            TextContent::Score { score } => self.resolve_score(score),
            TextContent::Nbt {
                nbt,
                interpret,
                plain: plain_style,
                separator,
                block,
                entity,
                storage,
            } => {
                let separator = self.resolve_optional(separator.as_deref().map(|s| &s.0), depth)?;
                self.resolve_nbt(
                    nbt,
                    *interpret,
                    *plain_style,
                    separator,
                    (block.as_deref(), entity.as_deref(), storage.as_deref()),
                    depth,
                )
            }
            other => Ok(plain(other.clone())),
        }
    }

    /// Vanilla `SelectorContents.resolve`.
    fn resolve_selector(
        &self,
        selector: &str,
        separator: Option<&str>,
    ) -> Result<TextComponentBase, CommandSyntaxError> {
        let entities = parse_selector(selector)?.find_entities(self.source)?;
        // `ComponentUtils.DEFAULT_SEPARATOR`.
        let separator = separator.map_or_else(
            || TextComponent::text(", ").color_named(NamedColor::Gray).0,
            |text| TextComponent::text(text.to_string()).0,
        );
        if entities.is_empty() {
            return Ok(empty());
        }
        let names = entities.iter().map(|e| e.get_display_name().0).collect();
        Ok(format_list(names, &separator))
    }

    /// Vanilla `ScoreContents.resolve`.
    fn resolve_score(&self, score: &ScoreContent) -> Result<TextComponentBase, CommandSyntaxError> {
        // `ScoreContents.findTargetName`: a selector, or else a plain score holder name.
        let holder = match parse_selector(&score.name) {
            Ok(selector) => {
                let entities = selector.find_entities(self.source)?;
                match entities.as_slice() {
                    [] => score.name.to_string(),
                    [entity] => entity.get_scoreboard_name(),
                    _ => return Err(entity::NOT_SINGLE_ENTITY_ERROR_TYPE.create_without_context()),
                }
            }
            Err(_) => score.name.to_string(),
        };
        // `ScoreHolder.WILDCARD` stands for the entity the text is resolved for.
        let holder = match self.default_scoreboard_entity {
            Some(entity) if holder == "*" => entity.get_scoreboard_name(),
            _ => holder,
        };
        let Some(server) = self.source.server.as_ref() else {
            return Ok(empty());
        };
        let scoreboard = server
            .scoreboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(objective) = scoreboard.get_objective(&score.objective) else {
            return Ok(empty());
        };
        let Some(entry) = scoreboard.get_score(&holder, &score.objective) else {
            return Ok(empty());
        };
        // `ReadOnlyScoreInfo.formatValue`: the score's own format, then the objective's.
        let value = entry.value.0.to_string();
        Ok(
            match entry
                .number_format
                .as_ref()
                .or(objective.number_format.as_ref())
            {
                Some(NumberFormat::Blank) => empty(),
                Some(NumberFormat::Fixed(text)) => text.0.clone(),
                Some(NumberFormat::Styled(style)) => {
                    let mut text = TextComponent::text(value).0;
                    text.style = Box::new(style.clone());
                    text
                }
                None => TextComponent::text(value).0,
            },
        )
    }

    /// The tags vanilla's `DataSource.getData` reads; a source that can't be read gives none.
    fn data(
        &self,
        (block, entity, storage): (Option<&str>, Option<&str>, Option<&str>),
    ) -> Vec<NbtTag> {
        let accessors: Vec<Box<dyn DataAccessor>> = match (entity, block, storage) {
            (Some(selector), _, _) => parse_selector(selector)
                .and_then(|selector| selector.find_entities(self.source))
                .map(|entities| {
                    entities
                        .into_iter()
                        .map(|entity| Box::new(EntityDataAccessor::new(entity)) as _)
                        .collect()
                })
                .unwrap_or_default(),
            (None, Some(pos), _) => {
                let mut reader = StringReader::new(pos);
                ArgumentType::<CommandSource>::parse(&BlockPosArgumentType, &mut reader)
                    .ok()
                    .zip(self.source.world.clone())
                    .and_then(|(coordinates, world)| {
                        BlockDataAccessor::new(
                            BlockPos::floored_v(coordinates.resolve(self.source)),
                            world,
                        )
                        .ok()
                    })
                    .map(|accessor| vec![Box::new(accessor) as _])
                    .unwrap_or_default()
            }
            (None, None, Some(id)) => self
                .source
                .server
                .iter()
                .map(|server| {
                    Box::new(StorageDataAccessor::new(id.to_string(), server.clone())) as _
                })
                .collect(),
            (None, None, None) => Vec::new(),
        };
        accessors
            .iter()
            .filter_map(|accessor| accessor.get_data().ok())
            .map(NbtTag::Compound)
            .collect()
    }

    /// Vanilla `NbtContents.resolve`.
    fn resolve_nbt(
        &self,
        path: &str,
        interpret: bool,
        plain_style: bool,
        separator: Option<TextComponentBase>,
        source: (Option<&str>, Option<&str>, Option<&str>),
        depth: usize,
    ) -> Result<TextComponentBase, CommandSyntaxError> {
        let count = self.resolved_component_count.get();
        if count > DEFAULT_RESOLVED_COMPONENT_LIMIT {
            return Ok(empty());
        }
        self.resolved_component_count.set(count + 1);
        let mut reader = StringReader::new(path);
        let Ok(path) = ArgumentType::<CommandSource>::parse(&NbtPathArgumentType, &mut reader)
        else {
            return Ok(empty());
        };
        let elements: Vec<NbtTag> = self
            .data(source)
            .iter()
            .flat_map(|tag| path.get(tag).unwrap_or_default())
            .collect();
        // `ComponentUtils.DEFAULT_NO_STYLE_SEPARATOR`.
        let separator = separator.unwrap_or_else(|| TextComponent::text(", ").0);
        let parts: Vec<TextComponentBase> = if interpret {
            elements
                .iter()
                .map(|tag| self.resolve(&TextComponent::from_nbt(tag).0, depth))
                .collect::<Result<_, _>>()?
        } else {
            elements
                .iter()
                .map(|tag| {
                    let rich = snbt_colorful_display(tag);
                    if plain_style {
                        TextComponent::text(Cow::Owned(rich.get_text())).0
                    } else {
                        rich.0
                    }
                })
                .collect()
        };
        // `reduce(left.append(separator).append(right))`.
        let mut parts = parts.into_iter();
        let Some(mut result) = parts.next() else {
            return Ok(empty());
        };
        for part in parts {
            result.extra.push(separator.clone());
            result.extra.push(part);
        }
        Ok(result)
    }
}
