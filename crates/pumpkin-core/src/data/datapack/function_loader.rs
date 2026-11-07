use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::TextComponent;
use tracing::error;

use crate::command::CommandSender;
use crate::command::commands::data::snbt_string;
use crate::command::context::command_context::ContextChain;
use crate::command::context::command_source::CommandSource;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::server::Server;

/// Vanilla `MacroFunction.MAX_CACHE_ENTRIES`.
const MAX_CACHE_ENTRIES: usize = 8;

/// One command of a function, parsed once (vanilla `UnboundEntryAction`). A command that
/// doesn't parse keeps only its text and reports the error each time it runs.
#[derive(Clone)]
pub struct FunctionCommand {
    pub text: Arc<str>,
    pub chain: Option<ContextChain<CommandSource>>,
}

/// The commands of a function ready to run.
pub type FunctionCommands = Arc<[FunctionCommand]>;

/// Parses function commands at `function-permission-level` against the current command
/// dispatcher (vanilla `ServerFunctionLibrary`'s compilation context).
pub struct FunctionCompiler {
    dispatcher: Arc<CommandDispatcher>,
    source: CommandSource,
    /// The source parsed commands keep until they run with their own: a stored command must
    /// not hold the server.
    unbound: Arc<CommandSource>,
}

impl FunctionCompiler {
    #[must_use]
    pub fn new(server: &Arc<Server>) -> Self {
        let mut source = CommandSender::Dummy.into_source(server).with_silent();
        source.permission_level = Some(server.basic_config.function_permission_level);
        Self {
            dispatcher: server.command_dispatcher.load_full(),
            source,
            unbound: Arc::new(CommandSource::dummy()),
        }
    }

    /// Vanilla `CommandFunction.parseCommand`.
    pub fn compile(&self, command: &str) -> Result<ContextChain<CommandSource>, String> {
        self.dispatcher
            .compile(command, &self.source)
            .map(|chain| chain.with_source(&self.unbound))
            .map_err(|error| error.message.get_text())
    }

    fn command(&self, text: &str) -> FunctionCommand {
        FunctionCommand {
            text: text.into(),
            chain: self.compile(text).ok(),
        }
    }
}

/// A loaded function (vanilla `CommandFunction`): its lines, and their commands parsed against
/// the command dispatcher they last ran with.
pub struct CommandFunction {
    lines: Box<[String]>,
    /// The macro variables in the order they first appear; empty for a plain function.
    parameters: Box<[String]>,
    compiled: Mutex<CompiledFunction>,
}

struct CompiledFunction {
    dispatcher: Arc<CommandDispatcher>,
    /// Macro lines are left unparsed here.
    commands: FunctionCommands,
    /// Vanilla `MacroFunction.cache`: the commands of recent argument values, newest last.
    macro_cache: VecDeque<(Box<[String]>, FunctionCommands)>,
}

impl CommandFunction {
    pub(crate) fn new(lines: Vec<FunctionLine>, dispatcher: Arc<CommandDispatcher>) -> Self {
        let mut parameters: Vec<String> = Vec::new();
        for template in lines.iter().filter_map(|(line, _)| macro_template(line)) {
            for variable in template.variables {
                if !parameters.iter().any(|known| known == variable) {
                    parameters.push(variable.to_string());
                }
            }
        }
        let commands = lines
            .iter()
            .map(|(line, chain)| FunctionCommand {
                text: line.as_str().into(),
                chain: chain.clone(),
            })
            .collect();
        Self {
            lines: lines.into_iter().map(|(line, _)| line).collect(),
            parameters: parameters.into(),
            compiled: Mutex::new(CompiledFunction {
                dispatcher,
                commands,
                macro_cache: VecDeque::new(),
            }),
        }
    }

    /// Whether the function has macro lines (vanilla `MacroFunction`).
    #[must_use]
    pub fn is_macro(&self) -> bool {
        !self.parameters.is_empty()
    }

    /// The function's lines as written, macro lines included.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Vanilla `CommandFunction.instantiate`: the function's commands with macro arguments
    /// filled in. Plain functions ignore `arguments`.
    pub fn instantiate(
        &self,
        id: &str,
        server: &Arc<Server>,
        arguments: Option<&NbtCompound>,
    ) -> Result<FunctionCommands, TextComponent> {
        let mut compiled = self
            .compiled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut compiler = None;
        if !Arc::ptr_eq(&compiled.dispatcher, &server.command_dispatcher.load()) {
            // The commands changed since the function was parsed: parse it again.
            let new = compiler.insert(FunctionCompiler::new(server));
            compiled.commands = self
                .lines
                .iter()
                .map(|line| {
                    if macro_template(line).is_some() {
                        FunctionCommand {
                            text: line.as_str().into(),
                            chain: None,
                        }
                    } else {
                        new.command(line)
                    }
                })
                .collect();
            compiled.dispatcher = new.dispatcher.clone();
            compiled.macro_cache.clear();
        }
        if self.parameters.is_empty() {
            return Ok(compiled.commands.clone());
        }

        let Some(arguments) = arguments else {
            return Err(TextComponent::translate_cross(
                translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS,
                translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS,
                [TextComponent::text(id.to_string())],
            ));
        };
        // Parameters are checked in the order they first appear.
        let mut values = Vec::with_capacity(self.parameters.len());
        for variable in &self.parameters {
            let Some(value) = arguments.get(variable) else {
                return Err(TextComponent::translate_cross(
                    translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT,
                    translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT,
                    [
                        TextComponent::text(id.to_string()),
                        TextComponent::text(variable.clone()),
                    ],
                ));
            };
            values.push(stringify_macro_argument(value));
        }
        if let Some(index) = compiled
            .macro_cache
            .iter()
            .position(|(key, _)| **key == *values)
            && let Some(entry) = compiled.macro_cache.remove(index)
        {
            let commands = entry.1.clone();
            compiled.macro_cache.push_back(entry);
            return Ok(commands);
        }

        let compiler = compiler.get_or_insert_with(|| FunctionCompiler::new(server));
        let by_name: HashMap<&str, &str> = self
            .parameters
            .iter()
            .map(String::as_str)
            .zip(values.iter().map(String::as_str))
            .collect();
        let commands: FunctionCommands = self
            .lines
            .iter()
            .zip(compiled.commands.iter())
            .map(|(line, command)| {
                macro_template(line).map_or_else(
                    || command.clone(),
                    |template| compiler.command(&template.substitute(&by_name)),
                )
            })
            .collect();
        if compiled.macro_cache.len() >= MAX_CACHE_ENTRIES {
            compiled.macro_cache.pop_front();
        }
        compiled
            .macro_cache
            .push_back((values.into(), commands.clone()));
        Ok(commands)
    }
}

/// A function line, with its parsed command unless it is a macro line.
pub(crate) type FunctionLine = (String, Option<ContextChain<CommandSource>>);

/// Reads the function files of a namespace, keyed by function id. They are compiled with
/// [`compile_functions`] once everything they may refer to has loaded.
pub fn load_functions_from_dir<S: std::hash::BuildHasher>(
    namespace: &str,
    function_dir: &Path,
    functions: &mut HashMap<String, String, S>,
) {
    if !function_dir.is_dir() {
        return;
    }
    load_functions_recursive(namespace, function_dir, function_dir, functions);
}

/// Vanilla `ServerFunctionLibrary.reload`: a function that fails to compile is logged and left
/// out.
pub fn compile_functions<S: std::hash::BuildHasher>(
    sources: HashMap<String, String, S>,
    compiler: &FunctionCompiler,
) -> HashMap<String, Arc<CommandFunction>> {
    let mut functions = HashMap::with_capacity(sources.len());
    for (id, content) in sources {
        match function_lines(&content, &|command| compiler.compile(command)) {
            Ok(lines) => {
                functions.insert(
                    id,
                    Arc::new(CommandFunction::new(lines, compiler.dispatcher.clone())),
                );
            }
            Err(reason) => error!("Failed to load function {id}: {reason}"),
        }
    }
    functions
}

fn load_functions_recursive<S: std::hash::BuildHasher>(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    functions: &mut HashMap<String, String, S>,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_functions_recursive(namespace, base_dir, &path, functions);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mcfunction"))
            && let Ok(rel_path) = path.strip_prefix(base_dir)
        {
            let mut stem_path = rel_path.to_string_lossy().to_string();
            if let Some(s) = stem_path.strip_suffix(".mcfunction") {
                stem_path = s.to_string();
            }
            // Convert Windows backslashes to forward slashes if any
            let stem_path = stem_path.replace('\\', "/");
            let function_id = format!("{namespace}:{stem_path}");
            if let Ok(content) = fs::read_to_string(&path) {
                functions.insert(function_id, content);
            }
        }
    }
}

/// The commands of a function file, like vanilla `CommandFunction.fromLines`: a line ending in
/// `\` continues on the next one, a macro line (`$`) must be a valid template, and every other
/// command must parse.
fn function_lines<T>(
    content: &str,
    compile: &dyn Fn(&str) -> Result<T, String>,
) -> Result<Vec<(String, Option<T>)>, String> {
    let mut lines = Vec::new();
    let mut raw = content.lines().map(str::trim).enumerate();
    while let Some((index, first)) = raw.next() {
        let line_number = index + 1;
        let mut line = first.to_string();
        while line.ends_with('\\') {
            line.pop();
            let Some((_, next)) = raw.next() else {
                return Err("Line continuation at end of file".to_string());
            };
            line.push_str(next);
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(command) = line.strip_prefix('/') {
            if command.starts_with('/') {
                return Err(format!(
                    "Unknown or invalid command '{line}' on line {line_number} (if you intended to make a comment, use '#' not '//')"
                ));
            }
            let name: String = command
                .chars()
                .take_while(|&c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
                .collect();
            return Err(format!(
                "Unknown or invalid command '{line}' on line {line_number} (did you mean '{name}'? Do not use a preceding forwards slash.)"
            ));
        }
        let command = if let Some(macro_line) = line.strip_prefix('$') {
            MacroTemplate::parse(macro_line)?;
            None
        } else {
            Some(compile(&line).map_err(|reason| {
                format!("Whilst parsing command on line {line_number}: {reason}")
            })?)
        };
        lines.push((line, command));
    }
    Ok(lines)
}

/// Vanilla `StringTemplate`: a macro line split around its `$(name)` variables.
struct MacroTemplate<'a> {
    segments: Vec<&'a str>,
    variables: Vec<&'a str>,
}

impl<'a> MacroTemplate<'a> {
    /// Vanilla `StringTemplate.fromString`.
    fn parse(input: &'a str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut variables = Vec::new();
        let mut start = 0;
        let mut search = 0;
        while let Some(offset) = input[search..].find('$') {
            let index = search + offset;
            if input[index + 1..].starts_with('(') {
                segments.push(&input[start..index]);
                let Some(end) = input[index + 1..].find(')').map(|end| index + 1 + end) else {
                    return Err("Unterminated macro variable".to_string());
                };
                let variable = &input[index + 2..end];
                if !variable.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return Err(format!("Invalid macro variable name '{variable}'"));
                }
                variables.push(variable);
                start = end + 1;
                search = start;
            } else {
                search = index + 1;
            }
        }
        if start == 0 {
            return Err("No variables in macro".to_string());
        }
        if start != input.len() {
            segments.push(&input[start..]);
        }
        Ok(Self {
            segments,
            variables,
        })
    }

    /// Vanilla `StringTemplate.substitute`.
    fn substitute(&self, values: &HashMap<&str, &str>) -> String {
        let mut command = String::new();
        for (i, variable) in self.variables.iter().enumerate() {
            command.push_str(self.segments[i]);
            command.push_str(values[variable]);
        }
        if self.segments.len() > self.variables.len() {
            command.push_str(self.segments[self.segments.len() - 1]);
        }
        command
    }
}

/// Vanilla `MacroFunction.stringify`: how a macro argument is written into the command.
fn stringify_macro_argument(tag: &NbtTag) -> String {
    // Vanilla's `DecimalFormat("#")` with at most 15 fraction digits.
    fn decimal(value: f64) -> String {
        let text = format!("{value:.15}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        // `#` drops a zero before the point, and the sign of a value that rounds to zero.
        let (sign, digits) = text.strip_prefix('-').map_or(("", text), |d| ("-", d));
        let digits = digits
            .strip_prefix('0')
            .filter(|d| !d.is_empty())
            .unwrap_or(digits);
        if digits == "0" {
            "0".to_string()
        } else {
            format!("{sign}{digits}")
        }
    }
    match tag {
        NbtTag::Float(value) => decimal(f64::from(*value)),
        NbtTag::Double(value) => decimal(*value),
        NbtTag::Byte(value) => value.to_string(),
        NbtTag::Short(value) => value.to_string(),
        NbtTag::Long(value) => value.to_string(),
        NbtTag::String(value) => value.to_string(),
        other => snbt_string(other),
    }
}

/// The template of a macro line (`$`), if it is one.
fn macro_template(line: &str) -> Option<MacroTemplate<'_>> {
    line.strip_prefix('$')
        .and_then(|line| MacroTemplate::parse(line).ok())
}

pub fn load_function_tags_from_dir<S: std::hash::BuildHasher>(
    namespace: &str,
    tags_dir: &Path,
    function_tags: &mut HashMap<String, Vec<String>, S>,
) {
    let function_tags_dir = tags_dir.join("function");
    let function_tags_dir_plural = tags_dir.join("functions");
    for dir in [&function_tags_dir, &function_tags_dir_plural] {
        if dir.is_dir() {
            load_tags_recursive(namespace, dir, dir, function_tags);
        }
    }
}

fn load_tags_recursive<S: std::hash::BuildHasher>(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    tags: &mut HashMap<String, Vec<String>, S>,
) {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_tags_recursive(namespace, base_dir, &path, tags);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            && let Ok(rel_path) = path.strip_prefix(base_dir)
        {
            let mut stem_path = rel_path.to_string_lossy().to_string();
            if let Some(s) = stem_path.strip_suffix(".json") {
                stem_path = s.to_string();
            }
            let stem_path = stem_path.replace('\\', "/");
            let tag_id = format!("{namespace}:{stem_path}");
            if let Ok(content) = fs::read_to_string(&path)
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(values_arr) = val.get("values").and_then(serde_json::Value::as_array)
            {
                let list: Vec<String> = values_arr
                    .iter()
                    .filter_map(|v| v.as_str().map(ToString::to_string))
                    .collect();
                tags.entry(tag_id).or_default().extend(list);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{function_lines, macro_template, stringify_macro_argument};
    use pumpkin_nbt::tag::NbtTag;
    use std::collections::HashMap;

    fn texts<T>(lines: Vec<(String, Option<T>)>) -> Vec<String> {
        lines.into_iter().map(|(line, _)| line).collect()
    }

    #[test]
    fn parse_mcfunction_content() {
        let content =
            "# This is a comment\nsay Hello world\n\n# Another comment\ngive @p stick 5\n";
        let lines = function_lines(content, &accept).expect("valid function");
        assert_eq!(texts(lines), ["say Hello world", "give @p stick 5"]);

        assert_eq!(
            function_lines("say hi\n/give @p diamond 1\n", &accept).map(texts),
            Err("Unknown or invalid command '/give @p diamond 1' on line 2 (did you mean 'give'? Do not use a preceding forwards slash.)".to_string())
        );
        let reject = |command: &str| {
            if command.starts_with("bad") {
                Err("Unknown command".to_string())
            } else {
                Ok(())
            }
        };
        assert_eq!(
            function_lines("say a \\\n b\nbad", &reject).map(texts),
            Err("Whilst parsing command on line 3: Unknown command".to_string())
        );
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "a compiler that accepts every command"
    )]
    fn accept(_: &str) -> Result<(), String> {
        Ok(())
    }

    #[test]
    fn continues_lines_and_fills_in_macros() {
        let lines = function_lines("say a \\\n  b\n$say $(x) and $(y)!\n", &accept)
            .expect("valid function");
        assert!(lines[0].1.is_some() && lines[1].1.is_none());
        assert_eq!(texts(lines), ["say a b", "$say $(x) and $(y)!"]);
        assert!(function_lines("$say no variables", &accept).is_err());
        assert!(function_lines("say end \\", &accept).is_err());

        let template = macro_template("$say $(x) and $(y)!").expect("a macro line");
        let x = stringify_macro_argument(&NbtTag::Float(0.5));
        let y = stringify_macro_argument(&NbtTag::String("hi".into()));
        let values = HashMap::from([("x", x.as_str()), ("y", y.as_str())]);
        assert_eq!(template.substitute(&values), "say .5 and hi!");
    }
}
