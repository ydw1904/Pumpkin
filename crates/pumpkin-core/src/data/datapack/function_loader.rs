use std::collections::HashMap;
use std::fs;
use std::path::Path;

use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::TextComponent;
use tracing::error;

use crate::command::commands::data::snbt_string;

pub fn load_functions_from_dir<S: std::hash::BuildHasher>(
    namespace: &str,
    function_dir: &Path,
    functions: &mut HashMap<String, Vec<String>, S>,
) {
    if !function_dir.is_dir() {
        return;
    }
    load_functions_recursive(namespace, function_dir, function_dir, functions);
}

fn load_functions_recursive<S: std::hash::BuildHasher>(
    namespace: &str,
    base_dir: &Path,
    current_dir: &Path,
    functions: &mut HashMap<String, Vec<String>, S>,
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
                match function_lines(&content) {
                    Ok(lines) => {
                        functions.insert(function_id, lines);
                    }
                    Err(reason) => error!("Failed to load function {function_id}: {reason}"),
                }
            }
        }
    }
}

/// The commands of a function file, like vanilla `CommandFunction.fromLines`: a line ending in
/// `\` continues on the next one, and a macro line (`$`) must be a valid template.
fn function_lines(content: &str) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut raw = content.lines().map(str::trim);
    while let Some(first) = raw.next() {
        let mut line = first.to_string();
        while line.ends_with('\\') {
            line.pop();
            let Some(next) = raw.next() else {
                return Err("Line continuation at end of file".to_string());
            };
            line.push_str(next);
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(macro_line) = line.strip_prefix('$') {
            MacroTemplate::parse(macro_line)?;
        }
        lines.push(line.strip_prefix('/').map_or(line.clone(), str::to_string));
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
    fn substitute(&self, values: &HashMap<&str, String>) -> String {
        let mut command = String::new();
        for (i, variable) in self.variables.iter().enumerate() {
            command.push_str(self.segments[i]);
            command.push_str(&values[variable]);
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

/// Vanilla `CommandFunction.instantiate`: a function's commands with macro arguments filled in.
/// Plain functions ignore `arguments`.
pub fn instantiate_function(
    id: &str,
    lines: &[String],
    arguments: Option<&NbtCompound>,
) -> Result<Vec<String>, TextComponent> {
    let templates: Vec<Option<MacroTemplate>> = lines
        .iter()
        .map(|line| {
            line.strip_prefix('$')
                .and_then(|line| MacroTemplate::parse(line).ok())
        })
        .collect();
    if templates.iter().all(Option::is_none) {
        return Ok(lines.to_vec());
    }
    let Some(arguments) = arguments else {
        return Err(TextComponent::translate_cross(
            translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS,
            translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS,
            [TextComponent::text(id.to_string())],
        ));
    };
    // Parameters are checked in the order they first appear.
    let mut values = HashMap::new();
    for variable in templates.iter().flatten().flat_map(|t| t.variables.iter()) {
        if values.contains_key(variable) {
            continue;
        }
        let Some(value) = arguments.get(variable) else {
            return Err(TextComponent::translate_cross(
                translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT,
                translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT,
                [
                    TextComponent::text(id.to_string()),
                    TextComponent::text((*variable).to_string()),
                ],
            ));
        };
        values.insert(*variable, stringify_macro_argument(value));
    }
    Ok(lines
        .iter()
        .zip(&templates)
        .map(|(line, template)| {
            template
                .as_ref()
                .map_or_else(|| line.clone(), |template| template.substitute(&values))
        })
        .collect())
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
    use super::{function_lines, instantiate_function};
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_nbt::tag::NbtTag;

    #[test]
    fn parse_mcfunction_content() {
        let content = "# This is a comment\nsay Hello world\n/give @p diamond 1\n\n# Another comment\ngive @p stick 5\n";
        let lines = function_lines(content).expect("valid function");

        assert_eq!(
            lines,
            vec![
                "say Hello world".to_string(),
                "give @p diamond 1".to_string(),
                "give @p stick 5".to_string()
            ]
        );
    }

    #[test]
    fn continues_lines_and_fills_in_macros() {
        let lines = function_lines("say a \\\n  b\n$say $(x) and $(y)!\n").expect("valid function");
        assert_eq!(lines, ["say a b", "$say $(x) and $(y)!"]);
        assert!(function_lines("$say no variables").is_err());
        assert!(function_lines("say end \\").is_err());

        let mut arguments = NbtCompound::new();
        arguments.put("x", NbtTag::Float(0.5));
        arguments.put("y", NbtTag::String("hi".into()));
        assert_eq!(
            instantiate_function("t:f", &lines, Some(&arguments)).expect("arguments given"),
            ["say a b", "say .5 and hi!"]
        );
        assert!(instantiate_function("t:f", &lines, None).is_err());
    }
}
