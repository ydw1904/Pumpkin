#![allow(
    clippy::too_many_lines,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::trivially_copy_pass_by_ref,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::missing_const_for_fn,
    clippy::redundant_closure_for_method_calls
)]

use std::fmt::Write;
use std::sync::Arc;

use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

use crate::block::entities::reload_block_entity;
use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::argument_types::core::double::DoubleArgumentType;
use crate::command::argument_types::core::integer::IntegerArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::identifier::IdentifierArgumentType;
use crate::command::argument_types::nbt::{NbtCompoundArgumentType, NbtTagArgumentType};
use crate::command::argument_types::nbt_path::{
    ERROR_DATA_TOO_DEEP, NbtPath, NbtPathArgumentType, is_too_deep,
};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::{java_double_string, java_float_string};
use crate::entity::EntityBase;
use crate::server::Server;
use crate::world::World;

const DESCRIPTION: &str = "Queries or modifies NBT data of entities, blocks, and storages.";
const PERMISSION: &str = "minecraft:command.data";

pub const ERROR_MERGE_UNCHANGED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_MERGE_FAILED,
    translation::java::COMMANDS_DATA_MERGE_FAILED,
);

pub const ERROR_GET_NOT_NUMBER: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_GET_INVALID,
    translation::java::COMMANDS_DATA_GET_INVALID,
);

pub const ERROR_GET_NON_EXISTENT: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_GET_UNKNOWN,
    translation::java::COMMANDS_DATA_GET_UNKNOWN,
);

pub const ERROR_MULTIPLE_TAGS: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_GET_MULTIPLE,
    translation::java::COMMANDS_DATA_GET_MULTIPLE,
);

pub const ERROR_EXPECTED_OBJECT: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_MODIFY_EXPECTED_OBJECT,
    translation::java::COMMANDS_DATA_MODIFY_EXPECTED_OBJECT,
);

pub const ERROR_EXPECTED_VALUE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_MODIFY_EXPECTED_VALUE,
    translation::java::COMMANDS_DATA_MODIFY_EXPECTED_VALUE,
);

pub const ERROR_INVALID_SUBSTRING: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_MODIFY_INVALID_SUBSTRING,
    translation::java::COMMANDS_DATA_MODIFY_INVALID_SUBSTRING,
);

pub const ERROR_BLOCK_INVALID: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_BLOCK_INVALID,
    translation::java::COMMANDS_DATA_BLOCK_INVALID,
);

pub const ERROR_ENTITY_INVALID: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_ENTITY_INVALID,
    translation::java::COMMANDS_DATA_ENTITY_INVALID,
);

/// Vanilla `StringTag.quoteAndEscape`: quotes with whichever quote mark the string doesn't
/// use first, preferring `"`.
fn quote_and_escape(input: &str) -> String {
    let mut body = String::with_capacity(input.len());
    let mut quote = None;
    for c in input.chars() {
        match c {
            '\\' => body.push_str("\\\\"),
            '"' | '\'' => {
                let quote = *quote.get_or_insert(if c == '"' { '\'' } else { '"' });
                if quote == c {
                    body.push('\\');
                }
                body.push(c);
            }
            '\u{8}' => body.push_str("\\b"),
            '\t' => body.push_str("\\t"),
            '\n' => body.push_str("\\n"),
            '\u{c}' => body.push_str("\\f"),
            '\r' => body.push_str("\\r"),
            c if c < ' ' => {
                let _ = write!(body, "\\x{:02X}", c as u32);
            }
            c => body.push(c),
        }
    }
    let quote = quote.unwrap_or('"');
    format!("{quote}{body}{quote}")
}

/// The order a Java `HashMap` filled one key at a time iterates its keys in, which is the order
/// vanilla prints compound keys in. Keys sharing a bucket keep insertion order in Java, which
/// is unknown here, so they fall back to name order.
fn java_hash_map_key_order(compound: &NbtCompound) -> Vec<&str> {
    let mut capacity: usize = 16;
    while compound.child_tags.len() > capacity / 4 * 3 {
        capacity *= 2;
    }
    let bucket = |key: &str| {
        let hash = key.encode_utf16().fold(0i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        }) as u32;
        (hash ^ (hash >> 16)) as usize & (capacity - 1)
    };
    let mut keys: Vec<&str> = compound.child_tags.keys().map(|key| &**key).collect();
    keys.sort_by(|a, b| bucket(a).cmp(&bucket(b)).then_with(|| a.cmp(b)));
    keys
}

/// Vanilla `Tag.toString` (`StringTagVisitor`), used where a tag is put into a message as text.
#[must_use]
pub fn snbt_string(tag: &NbtTag) -> String {
    fn join<T>(items: &[T], format: impl Fn(&T) -> String) -> String {
        items.iter().map(format).collect::<Vec<_>>().join(",")
    }
    match tag {
        NbtTag::End => "END".to_string(),
        NbtTag::Byte(value) => format!("{value}b"),
        NbtTag::Short(value) => format!("{value}s"),
        NbtTag::Int(value) => value.to_string(),
        NbtTag::Long(value) => format!("{value}L"),
        NbtTag::Float(value) => format!("{}f", java_float_string(*value)),
        NbtTag::Double(value) => format!("{}d", java_double_string(*value)),
        NbtTag::String(value) => quote_and_escape(value),
        NbtTag::ByteArray(values) => format!("[B;{}]", join(values, |v| format!("{v}B"))),
        NbtTag::IntArray(values) => format!("[I;{}]", join(values, ToString::to_string)),
        NbtTag::LongArray(values) => format!("[L;{}]", join(values, |v| format!("{v}L"))),
        NbtTag::List(values) => format!("[{}]", join(values, snbt_string)),
        NbtTag::Compound(compound) => {
            let mut keys: Vec<&str> = compound.child_tags.keys().map(|key| &**key).collect();
            keys.sort_unstable();
            let entries = join(&keys, |key| {
                let unquoted = !key.eq_ignore_ascii_case("true")
                    && !key.eq_ignore_ascii_case("false")
                    && key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '.' || c == '_')
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
                let key_text = if unquoted {
                    (*key).to_string()
                } else {
                    quote_and_escape(key)
                };
                format!("{key_text}:{}", snbt_string(&compound.child_tags[*key]))
            });
            format!("{{{entries}}}")
        }
    }
}

/// Vanilla `NbtUtils.toPrettyComponent`: `TextComponentTagVisitor` with no indentation.
#[must_use]
pub fn snbt_colorful_display(tag: &NbtTag) -> TextComponent {
    let mut visitor = PrettyTagVisitor {
        result: TextComponent::empty(),
        depth: 0,
    };
    visitor.visit(tag);
    visitor.result
}

/// Vanilla `TextComponentTagVisitor.MAX_DEPTH`.
const MAX_DEPTH: usize = 64;
/// Vanilla `TextComponentTagVisitor.MAX_LENGTH`.
const MAX_LENGTH: usize = 128;
/// Vanilla `TextComponentTagVisitor.INLINE_LIST_THRESHOLD`.
const INLINE_LIST_THRESHOLD: usize = 8;

struct PrettyTagVisitor {
    result: TextComponent,
    depth: usize,
}

impl PrettyTagVisitor {
    fn append(&mut self, component: TextComponent) {
        self.result =
            std::mem::replace(&mut self.result, TextComponent::empty()).add_child(component);
    }

    fn plain(&mut self, text: &str) {
        self.append(TextComponent::text(text.to_string()));
    }

    fn colored(&mut self, text: String, color: NamedColor) {
        self.append(TextComponent::text(text).color_named(color));
    }

    fn number(&mut self, value: String, suffix: &str) {
        self.colored(value, NamedColor::Gold);
        if !suffix.is_empty() {
            self.colored(suffix.to_string(), NamedColor::Red);
        }
    }

    fn folded(&mut self) {
        self.colored("<...>".to_string(), NamedColor::Gray);
    }

    fn array<T: ToString>(&mut self, prefix: &str, values: &[T], suffix: &str) {
        self.plain("[");
        self.colored(prefix.to_string(), NamedColor::Red);
        self.plain(";");
        for (i, value) in values.iter().take(MAX_LENGTH).enumerate() {
            self.plain(" ");
            self.number(value.to_string(), suffix);
            if i != values.len() - 1 {
                self.plain(",");
            }
        }
        if values.len() > MAX_LENGTH {
            self.folded();
        }
        self.plain("]");
    }

    /// Only the text between the quotes is colored.
    fn quoted(&mut self, quoted: &str, color: NamedColor) {
        let quote = &quoted[..1];
        let inner = &quoted[1..quoted.len() - 1];
        self.plain(quote);
        self.colored(inner.to_string(), color);
        self.plain(quote);
    }

    fn sub_tag(&mut self, tag: &NbtTag) {
        self.depth += 1;
        self.visit(tag);
        self.depth -= 1;
    }

    fn visit(&mut self, tag: &NbtTag) {
        match tag {
            NbtTag::End => {}
            NbtTag::Byte(value) => self.number(value.to_string(), "b"),
            NbtTag::Short(value) => self.number(value.to_string(), "s"),
            NbtTag::Int(value) => self.number(value.to_string(), ""),
            NbtTag::Long(value) => self.number(value.to_string(), "L"),
            NbtTag::Float(value) => self.number(java_float_string(*value), "f"),
            NbtTag::Double(value) => self.number(java_double_string(*value), "d"),
            NbtTag::String(value) => self.quoted(&quote_and_escape(value), NamedColor::Green),
            NbtTag::ByteArray(values) => self.array("B", values, "b"),
            NbtTag::IntArray(values) => self.array("I", values, ""),
            NbtTag::LongArray(values) => self.array("L", values, "L"),
            NbtTag::List(values) => {
                self.plain("[");
                if values.is_empty() {
                    // `[]`
                } else if self.depth >= MAX_DEPTH {
                    self.folded();
                } else if values.len() >= INLINE_LIST_THRESHOLD
                    || values.iter().all(|value| {
                        matches!(
                            value,
                            NbtTag::Byte(_)
                                | NbtTag::Short(_)
                                | NbtTag::Int(_)
                                | NbtTag::Long(_)
                                | NbtTag::Float(_)
                                | NbtTag::Double(_)
                        )
                    })
                {
                    // Vanilla doesn't fold inline lists.
                    for (i, value) in values.iter().enumerate() {
                        if i != 0 {
                            self.plain(", ");
                        }
                        self.sub_tag(value);
                    }
                } else {
                    for (i, value) in values.iter().take(MAX_LENGTH).enumerate() {
                        self.sub_tag(value);
                        if i != values.len() - 1 {
                            self.plain(", ");
                        }
                    }
                    if values.len() > MAX_LENGTH {
                        self.folded();
                    }
                }
                self.plain("]");
            }
            NbtTag::Compound(compound) => {
                self.plain("{");
                if compound.is_empty() {
                    // `{}`
                } else if self.depth >= MAX_DEPTH {
                    self.folded();
                } else {
                    for (i, key) in java_hash_map_key_order(compound).into_iter().enumerate() {
                        if i != 0 {
                            self.plain(", ");
                        }
                        let unquoted = key.chars().all(|c| {
                            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')
                        });
                        if unquoted {
                            self.colored(key.to_string(), NamedColor::Aqua);
                        } else {
                            self.quoted(&quote_and_escape(key), NamedColor::Aqua);
                        }
                        self.plain(": ");
                        self.sub_tag(&compound.child_tags[key]);
                    }
                }
                self.plain("}");
            }
        }
    }
}

// ---------------- DATA ACCESSOR ABSTRACTION ----------------

pub trait DataAccessor: Send + Sync {
    fn set_data(&self, data: &NbtCompound) -> Result<(), CommandSyntaxError>;
    /// `data` is the full snapshot without the `removed` top-level keys.
    fn remove_data(
        &self,
        data: &NbtCompound,
        _removed: &[Box<str>],
    ) -> Result<(), CommandSyntaxError> {
        self.set_data(data)
    }
    fn get_data(&self) -> Result<NbtCompound, CommandSyntaxError>;
    fn get_modified_success(&self) -> TextComponent;
    fn get_print_success(&self, data: &NbtTag) -> TextComponent;
    fn get_print_scaled_success(&self, path: &NbtPath, scale: f64, value: i32) -> TextComponent;
}

pub struct EntityDataAccessor {
    entity: Arc<dyn EntityBase>,
}

impl EntityDataAccessor {
    #[must_use]
    pub fn new(entity: Arc<dyn EntityBase>) -> Self {
        Self { entity }
    }
}

impl DataAccessor for EntityDataAccessor {
    fn set_data(&self, tag: &NbtCompound) -> Result<(), CommandSyntaxError> {
        if self.entity.get_player().is_some() {
            return Err(ERROR_ENTITY_INVALID.create_without_context());
        }
        self.entity.read_nbt_non_mut(tag);
        Ok(())
    }

    fn remove_data(
        &self,
        data: &NbtCompound,
        removed: &[Box<str>],
    ) -> Result<(), CommandSyntaxError> {
        self.set_data(data)?;
        self.entity.remove_nbt(removed);
        Ok(())
    }

    fn get_data(&self) -> Result<NbtCompound, CommandSyntaxError> {
        let mut nbt = NbtCompound::new();
        self.entity.write_nbt(&mut nbt);
        Ok(nbt)
    }

    fn get_modified_success(&self) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_ENTITY_MODIFIED,
            translation::java::COMMANDS_DATA_ENTITY_MODIFIED,
            [self.entity.get_display_name()],
        )
    }

    fn get_print_success(&self, data: &NbtTag) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_ENTITY_QUERY,
            translation::java::COMMANDS_DATA_ENTITY_QUERY,
            [self.entity.get_display_name(), snbt_colorful_display(data)],
        )
    }

    fn get_print_scaled_success(&self, path: &NbtPath, scale: f64, value: i32) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_ENTITY_GET,
            translation::java::COMMANDS_DATA_ENTITY_GET,
            [
                TextComponent::text(path.as_str().to_string()),
                self.entity.get_display_name(),
                TextComponent::text(format!("{scale:.2}")),
                TextComponent::text(value.to_string()),
            ],
        )
    }
}

pub struct BlockDataAccessor {
    pos: BlockPos,
    world: Arc<World>,
}

impl BlockDataAccessor {
    pub fn new(pos: BlockPos, world: Arc<World>) -> Result<Self, CommandSyntaxError> {
        if world.get_block_entity(&pos).is_none() {
            return Err(ERROR_BLOCK_INVALID.create_without_context());
        }
        Ok(Self { pos, world })
    }

    /// The x, y and z message arguments vanilla passes separately.
    fn pos_args(&self) -> [TextComponent; 3] {
        let BlockPos(pos) = self.pos;
        [pos.x, pos.y, pos.z].map(|coordinate| TextComponent::text(coordinate.to_string()))
    }
}

impl DataAccessor for BlockDataAccessor {
    fn set_data(&self, tag: &NbtCompound) -> Result<(), CommandSyntaxError> {
        let Some(entity) = self.world.get_block_entity(&self.pos) else {
            return Err(ERROR_BLOCK_INVALID.create_without_context());
        };
        // `add_block_entity` also sends the update tag, like vanilla `sendBlockUpdated`.
        if let Some(loaded) = reload_block_entity(entity.as_ref(), tag) {
            self.world.add_block_entity(loaded);
            self.world.block_entity_set_changed(&self.pos);
        }
        Ok(())
    }

    fn get_data(&self) -> Result<NbtCompound, CommandSyntaxError> {
        if let Some(block_entity) = self.world.get_block_entity(&self.pos) {
            let mut nbt = NbtCompound::new();
            block_entity.write_internal(&mut nbt);
            Ok(nbt)
        } else {
            Err(ERROR_BLOCK_INVALID.create_without_context())
        }
    }

    fn get_modified_success(&self) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_BLOCK_MODIFIED,
            translation::java::COMMANDS_DATA_BLOCK_MODIFIED,
            self.pos_args(),
        )
    }

    fn get_print_success(&self, data: &NbtTag) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_BLOCK_QUERY,
            translation::java::COMMANDS_DATA_BLOCK_QUERY,
            [self.pos_args().as_slice(), &[snbt_colorful_display(data)]].concat(),
        )
    }

    fn get_print_scaled_success(&self, path: &NbtPath, scale: f64, value: i32) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_BLOCK_GET,
            translation::java::COMMANDS_DATA_BLOCK_GET,
            [
                TextComponent::text(path.as_str().to_string()),
                TextComponent::text(format!("{}", self.pos.0.x)),
                TextComponent::text(format!("{}", self.pos.0.y)),
                TextComponent::text(format!("{}", self.pos.0.z)),
                TextComponent::text(format!("{scale:.2}")),
                TextComponent::text(value.to_string()),
            ],
        )
    }
}

pub struct StorageDataAccessor {
    id: String,
    server: Arc<Server>,
}

impl StorageDataAccessor {
    #[must_use]
    pub fn new(id: String, server: Arc<Server>) -> Self {
        Self { id, server }
    }
}

impl DataAccessor for StorageDataAccessor {
    fn set_data(&self, tag: &NbtCompound) -> Result<(), CommandSyntaxError> {
        let mut storage = self
            .server
            .command_storage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        storage.insert(self.id.clone(), tag.clone());
        Ok(())
    }

    fn get_data(&self) -> Result<NbtCompound, CommandSyntaxError> {
        let storage = self
            .server
            .command_storage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(storage.get(&self.id).cloned().unwrap_or_default())
    }

    fn get_modified_success(&self) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_STORAGE_MODIFIED,
            translation::java::COMMANDS_DATA_STORAGE_MODIFIED,
            [TextComponent::text(self.id.clone())],
        )
    }

    fn get_print_success(&self, data: &NbtTag) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_STORAGE_QUERY,
            translation::java::COMMANDS_DATA_STORAGE_QUERY,
            [
                TextComponent::text(self.id.clone()),
                snbt_colorful_display(data),
            ],
        )
    }

    fn get_print_scaled_success(&self, path: &NbtPath, scale: f64, value: i32) -> TextComponent {
        TextComponent::translate_cross(
            translation::java::COMMANDS_DATA_STORAGE_GET,
            translation::java::COMMANDS_DATA_STORAGE_GET,
            [
                TextComponent::text(path.as_str().to_string()),
                TextComponent::text(self.id.clone()),
                TextComponent::text(format!("{scale:.2}")),
                TextComponent::text(value.to_string()),
            ],
        )
    }
}

pub fn get_single_tag(
    path: &NbtPath,
    accessor: &dyn DataAccessor,
) -> Result<NbtTag, CommandSyntaxError> {
    let data = accessor.get_data()?;
    let root = NbtTag::Compound(data);
    let tags = path.get(&root)?;
    match tags.len() {
        0 => Err(ERROR_GET_NON_EXISTENT
            .create_without_context(TextComponent::text(path.as_str().to_string()))),
        1 => Ok(tags.into_iter().next().unwrap()),
        _ => Err(ERROR_MULTIPLE_TAGS.create_without_context()),
    }
}

fn get_as_text(tag: &NbtTag) -> Result<String, CommandSyntaxError> {
    match tag {
        NbtTag::String(s) => Ok(s.to_string()),
        NbtTag::Byte(_)
        | NbtTag::Short(_)
        | NbtTag::Int(_)
        | NbtTag::Long(_)
        | NbtTag::Float(_)
        | NbtTag::Double(_) => Ok(snbt_string(tag)),
        _ => {
            Err(ERROR_EXPECTED_VALUE.create_without_context(TextComponent::text(snbt_string(tag))))
        }
    }
}

fn substring(input: &str, start: i32, end: Option<i32>) -> Result<String, CommandSyntaxError> {
    let len = input.chars().count() as i32;
    let abs_start = if start >= 0 { start } else { len + start };
    let abs_end = match end {
        Some(e) => {
            if e >= 0 {
                e
            } else {
                len + e
            }
        }
        None => len,
    };
    if abs_start >= 0 && abs_end <= len && abs_start <= abs_end {
        let chars: Vec<char> = input.chars().collect();
        Ok(chars[(abs_start as usize)..(abs_end as usize)]
            .iter()
            .collect())
    } else {
        Err(ERROR_INVALID_SUBSTRING.create_without_context(
            TextComponent::text(start.to_string()),
            TextComponent::text(end.unwrap_or(len).to_string()),
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Entity,
    Block,
    Storage,
}

/// The data sources and targets in vanilla's `DataCommands.ALL_PROVIDERS` order.
const TARGET_KINDS: [(TargetKind, &str); 3] = [
    (TargetKind::Entity, "entity"),
    (TargetKind::Block, "block"),
    (TargetKind::Storage, "storage"),
];

impl TargetKind {
    pub fn access(
        &self,
        context: &CommandContext,
        prefix: &str,
    ) -> Result<Box<dyn DataAccessor>, CommandSyntaxError> {
        match self {
            Self::Entity => {
                let entity = EntityArgumentType::get_entity(context, prefix)?;
                Ok(Box::new(EntityDataAccessor::new(entity)))
            }
            Self::Block => {
                let pos = BlockPosArgumentType::get_block_pos(context, prefix)?;
                let world = context.world().clone();
                let accessor = BlockDataAccessor::new(pos, world)?;
                Ok(Box::new(accessor))
            }
            Self::Storage => {
                let id = context.get_argument::<pumpkin_util::identifier::Identifier>(prefix)?;
                let server = context.server().clone();
                Ok(Box::new(StorageDataAccessor::new(id.to_string(), server)))
            }
        }
    }
}

// ---------------- EXECUTION LOGIC ----------------

fn execute_get(
    context: &CommandContext,
    target_kind: TargetKind,
    has_path: bool,
    has_scale: bool,
) -> CommandExecutorResult {
    let accessor = target_kind.access(context, "target")?;
    if !has_path {
        let data = accessor.get_data()?;
        context
            .source
            .send_feedback(accessor.get_print_success(&NbtTag::Compound(data)), false);
        return Ok(1);
    }

    let path = context.get_argument::<NbtPath>("path")?;
    if has_scale {
        let scale = DoubleArgumentType::get(context, "scale")?;
        let tag = get_single_tag(path, accessor.as_ref())?;
        let num = match &tag {
            NbtTag::Byte(b) => *b as f64,
            NbtTag::Short(s) => *s as f64,
            NbtTag::Int(i) => *i as f64,
            NbtTag::Long(l) => *l as f64,
            NbtTag::Float(f) => *f as f64,
            NbtTag::Double(d) => *d,
            _ => {
                return Err(ERROR_GET_NOT_NUMBER
                    .create_without_context(TextComponent::text(path.as_str().to_string())));
            }
        };
        let result = (num * scale).floor() as i32;
        context.source.send_feedback(
            accessor.get_print_scaled_success(path, scale, result),
            false,
        );
        return Ok(result);
    }

    let tag = get_single_tag(path, accessor.as_ref())?;
    let result = match &tag {
        NbtTag::Byte(b) => *b as i32,
        NbtTag::Short(s) => *s as i32,
        NbtTag::Int(i) => *i,
        NbtTag::Long(l) => *l as i32,
        NbtTag::Float(f) => f.floor() as i32,
        NbtTag::Double(d) => d.floor() as i32,
        NbtTag::ByteArray(arr) => arr.len() as i32,
        NbtTag::IntArray(arr) => arr.len() as i32,
        NbtTag::LongArray(arr) => arr.len() as i32,
        // Java `String.length` counts UTF-16 code units.
        NbtTag::String(s) => s.encode_utf16().count() as i32,
        NbtTag::List(list) => list.len() as i32,
        NbtTag::Compound(compound) => compound.child_tags.len() as i32,
        NbtTag::End => {
            return Err(ERROR_GET_NON_EXISTENT
                .create_without_context(TextComponent::text(path.as_str().to_string())));
        }
    };
    context
        .source
        .send_feedback(accessor.get_print_success(&tag), false);
    Ok(result)
}

fn execute_merge(context: &CommandContext, target_kind: TargetKind) -> CommandExecutorResult {
    let accessor = target_kind.access(context, "target")?;
    let nbt = NbtCompoundArgumentType::get(context, "nbt")?.clone();
    if is_too_deep(&NbtTag::Compound(nbt.clone()), 0) {
        return Err(ERROR_DATA_TOO_DEEP.create_without_context());
    }
    let old = accessor.get_data()?;
    let mut updated = old.clone();
    updated.merge(&nbt);
    if old == updated {
        return Err(ERROR_MERGE_UNCHANGED.create_without_context());
    }
    accessor.set_data(&updated)?;
    context
        .source
        .send_feedback(accessor.get_modified_success(), true);
    Ok(1)
}

fn execute_remove(context: &CommandContext, target_kind: TargetKind) -> CommandExecutorResult {
    let accessor = target_kind.access(context, "target")?;
    let path = context.get_argument::<NbtPath>("path")?;
    let old = accessor.get_data()?;
    let mut root = NbtTag::Compound(old.clone());
    let count = path.remove(&mut root);
    if count == 0 {
        return Err(ERROR_MERGE_UNCHANGED.create_without_context());
    }
    let NbtTag::Compound(data) = root else {
        return Err(ERROR_MERGE_UNCHANGED.create_without_context());
    };
    let removed: Vec<Box<str>> = old
        .child_tags
        .into_keys()
        .filter(|key| !data.has(key))
        .collect();
    accessor.remove_data(&data, &removed)?;
    context
        .source
        .send_feedback(accessor.get_modified_success(), true);
    Ok(count)
}

#[derive(Clone, Copy)]
pub enum ModifyMode {
    Insert(i32),
    Prepend,
    Append,
    Set,
    Merge,
}

#[derive(Clone, Copy)]
pub enum SourceMode {
    From {
        has_path: bool,
    },
    String {
        has_path: bool,
        has_start: bool,
        has_end: bool,
    },
    Value,
}

fn resolve_source_tags(
    context: &CommandContext,
    source_kind: Option<TargetKind>,
    source_mode: SourceMode,
) -> Result<Vec<NbtTag>, CommandSyntaxError> {
    match source_mode {
        SourceMode::Value => {
            let val = NbtTagArgumentType::get(context, "value")?.clone();
            Ok(vec![val])
        }
        SourceMode::From { has_path } => {
            let source_accessor = source_kind.unwrap().access(context, "source")?;
            let data = source_accessor.get_data()?;
            if has_path {
                let source_path = context.get_argument::<NbtPath>("sourcePath")?;
                source_path.get(&NbtTag::Compound(data))
            } else {
                Ok(vec![NbtTag::Compound(data)])
            }
        }
        SourceMode::String {
            has_path,
            has_start,
            has_end,
        } => {
            let source_accessor = source_kind.unwrap().access(context, "source")?;
            let data = source_accessor.get_data()?;
            let tags = if has_path {
                let source_path = context.get_argument::<NbtPath>("sourcePath")?;
                source_path.get(&NbtTag::Compound(data))?
            } else {
                vec![NbtTag::Compound(data)]
            };

            let start = if has_start {
                IntegerArgumentType::get(context, "start")?
            } else {
                0
            };
            let end = if has_end {
                Some(IntegerArgumentType::get(context, "end")?)
            } else {
                None
            };

            let mut string_tags = Vec::with_capacity(tags.len());
            for tag in tags {
                let text = get_as_text(&tag)?;
                let sub = substring(&text, start, end)?;
                string_tags.push(NbtTag::String(sub.into_boxed_str()));
            }
            Ok(string_tags)
        }
    }
}

fn execute_modify(
    context: &CommandContext,
    target_kind: TargetKind,
    modify_mode: ModifyMode,
    source_kind: Option<TargetKind>,
    source_mode: SourceMode,
) -> CommandExecutorResult {
    let target_accessor = target_kind.access(context, "target")?;
    let target_path = context.get_argument::<NbtPath>("targetPath")?;
    let mut target_data = target_accessor.get_data()?;
    let mut root = NbtTag::Compound(target_data.clone());
    let source_tags = resolve_source_tags(context, source_kind, source_mode)?;

    let result = match modify_mode {
        ModifyMode::Insert(idx) => target_path.insert(idx, &mut root, &source_tags)?,
        ModifyMode::Prepend => target_path.insert(0, &mut root, &source_tags)?,
        ModifyMode::Append => target_path.insert(-1, &mut root, &source_tags)?,
        ModifyMode::Set => {
            let last_tag = source_tags.last().cloned().unwrap_or(NbtTag::End);
            target_path.set(&mut root, last_tag)?
        }
        ModifyMode::Merge => {
            let mut combined = NbtCompound::new();
            for tag in &source_tags {
                if is_too_deep(tag, 0) {
                    return Err(ERROR_DATA_TOO_DEEP.create_without_context());
                }
                let NbtTag::Compound(compound) = tag else {
                    return Err(ERROR_EXPECTED_OBJECT
                        .create_without_context(TextComponent::text(snbt_string(tag))));
                };
                combined.merge(compound);
            }

            target_path.get_or_create(
                &mut root,
                &|| NbtTag::Compound(NbtCompound::new()),
                |target| {
                    let NbtTag::Compound(target_object) = target else {
                        return Err(ERROR_EXPECTED_OBJECT
                            .create_without_context(TextComponent::text(snbt_string(target))));
                    };
                    let original = target_object.clone();
                    target_object.merge(&combined);
                    Ok(i32::from(original != *target_object))
                },
            )?
        }
    };

    if result == 0 {
        return Err(ERROR_MERGE_UNCHANGED.create_without_context());
    }

    if let NbtTag::Compound(new_compound) = root {
        target_data = new_compound;
    }
    target_accessor.set_data(&target_data)?;
    context
        .source
        .send_feedback(target_accessor.get_modified_success(), true);
    Ok(result)
}

// ---------------- EXECUTOR STRUCTS ----------------

struct GetExecutor {
    target_kind: TargetKind,
    has_path: bool,
    has_scale: bool,
}

impl CommandExecutor for GetExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        execute_get(context, self.target_kind, self.has_path, self.has_scale)
    }
}

struct MergeExecutor {
    target_kind: TargetKind,
}

impl CommandExecutor for MergeExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        execute_merge(context, self.target_kind)
    }
}

struct RemoveExecutor {
    target_kind: TargetKind,
}

impl CommandExecutor for RemoveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        execute_remove(context, self.target_kind)
    }
}

struct ModifyExecutor {
    target_kind: TargetKind,
    modify_mode: ModifyMode,
    source_kind: Option<TargetKind>,
    source_mode: SourceMode,
}

impl CommandExecutor for ModifyExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let modify_mode = match self.modify_mode {
            ModifyMode::Insert(_) => {
                let idx = IntegerArgumentType::get(context, "index")?;
                ModifyMode::Insert(idx)
            }
            other => other,
        };
        execute_modify(
            context,
            self.target_kind,
            modify_mode,
            self.source_kind,
            self.source_mode,
        )
    }
}

// ---------------- REGISTRATION ----------------

pub fn make_target_arg(
    kind: TargetKind,
    name: &'static str,
) -> crate::command::argument_builder::RequiredArgumentBuilder {
    match kind {
        TargetKind::Entity => argument(name, EntityArgumentType::Entity),
        TargetKind::Block => argument(name, BlockPosArgumentType),
        TargetKind::Storage => argument(name, IdentifierArgumentType),
    }
}

/// Adds the `value`, `from` and `string` sources of `data modify` under `node`.
fn add_modify_sources<B: ArgumentBuilder>(
    node: B,
    target_kind: TargetKind,
    modify_mode: ModifyMode,
) -> B {
    let executor = |source_kind, source_mode| ModifyExecutor {
        target_kind,
        modify_mode,
        source_kind,
        source_mode,
    };
    let string_mode = |has_path, has_start, has_end| SourceMode::String {
        has_path,
        has_start,
        has_end,
    };

    // value <value>
    let node =
        node.then(literal("value").then(
            argument("value", NbtTagArgumentType).executes(executor(None, SourceMode::Value)),
        ));

    // `then` replaces a child with the same name, so each literal collects every source kind.
    let mut from_node = literal("from");
    let mut string_node = literal("string");
    for (source_kind, source_name) in TARGET_KINDS {
        let source = Some(source_kind);
        // from <source> [<sourcePath>]
        from_node = from_node.then(
            literal(source_name).then(
                make_target_arg(source_kind, "source")
                    .executes(executor(source, SourceMode::From { has_path: false }))
                    .then(
                        argument("sourcePath", NbtPathArgumentType)
                            .executes(executor(source, SourceMode::From { has_path: true })),
                    ),
            ),
        );

        // string <source> [<sourcePath>] [<start>] [<end>]
        string_node = string_node.then(
            literal(source_name).then(
                make_target_arg(source_kind, "source")
                    .executes(executor(source, string_mode(false, false, false)))
                    .then(
                        argument("sourcePath", NbtPathArgumentType)
                            .executes(executor(source, string_mode(true, false, false)))
                            .then(
                                argument("start", IntegerArgumentType::any())
                                    .executes(executor(source, string_mode(true, true, false)))
                                    .then(
                                        argument("end", IntegerArgumentType::any()).executes(
                                            executor(source, string_mode(true, true, true)),
                                        ),
                                    ),
                            ),
                    ),
            ),
        );
    }
    node.then(from_node).then(string_node)
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let mut data_cmd = command("data", DESCRIPTION).requires(PERMISSION);

    // Merge & Get & Remove
    // Each verb literal is built once and shared across target kinds; `.then()` on `data_cmd`
    // itself replaces same-named children instead of merging them, so building fresh
    // `literal("get")` etc. per target kind would silently drop every kind but the last.
    let mut merge_node = literal("merge");
    let mut get_node = literal("get");
    let mut remove_node = literal("remove");
    for (target_kind, name) in TARGET_KINDS {
        // data merge <target> <nbt>
        merge_node = merge_node.then(literal(name).then(
            make_target_arg(target_kind, "target").then(
                argument("nbt", NbtCompoundArgumentType).executes(MergeExecutor { target_kind }),
            ),
        ));

        // data get <target> [<path>] [<scale>]
        get_node = get_node.then(
            literal(name).then(
                make_target_arg(target_kind, "target")
                    .executes(GetExecutor {
                        target_kind,
                        has_path: false,
                        has_scale: false,
                    })
                    .then(
                        argument("path", NbtPathArgumentType)
                            .executes(GetExecutor {
                                target_kind,
                                has_path: true,
                                has_scale: false,
                            })
                            .then(argument("scale", DoubleArgumentType::any()).executes(
                                GetExecutor {
                                    target_kind,
                                    has_path: true,
                                    has_scale: true,
                                },
                            )),
                    ),
            ),
        );

        // data remove <target> <path>
        remove_node = remove_node.then(literal(name).then(
            make_target_arg(target_kind, "target").then(
                argument("path", NbtPathArgumentType).executes(RemoveExecutor { target_kind }),
            ),
        ));
    }
    data_cmd = data_cmd.then(merge_node).then(get_node).then(remove_node);

    // data modify <target> <targetPath> (insert <index> | prepend | append | set | merge) ...
    let modify_modes = [
        ("insert", ModifyMode::Insert(0)),
        ("prepend", ModifyMode::Prepend),
        ("append", ModifyMode::Append),
        ("set", ModifyMode::Set),
        ("merge", ModifyMode::Merge),
    ];

    let mut modify_node = literal("modify");
    for (target_kind, target_name) in TARGET_KINDS {
        let mut target_path_arg = argument("targetPath", NbtPathArgumentType);
        for &(mod_name, modify_mode) in &modify_modes {
            let mod_node = if mod_name == "insert" {
                literal("insert").then(add_modify_sources(
                    argument("index", IntegerArgumentType::any()),
                    target_kind,
                    modify_mode,
                ))
            } else {
                add_modify_sources(literal(mod_name), target_kind, modify_mode)
            };
            target_path_arg = target_path_arg.then(mod_node);
        }

        modify_node = modify_node.then(
            literal(target_name).then(make_target_arg(target_kind, "target").then(target_path_arg)),
        );
    }
    data_cmd = data_cmd.then(modify_node);

    dispatcher.register(data_cmd);
}
