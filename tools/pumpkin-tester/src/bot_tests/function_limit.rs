use std::{fs, sync::Arc};

use pumpkin_core::{command::CommandSender, server::Server};
use pumpkin_data::dimension::Dimension;

/// Functions that call themselves forever used to recurse on the stack until the server
/// crashed. Vanilla runs them from a flat queue and stops at `max_command_sequence_length`.
pub fn run_function_limit_test(
    server: &Arc<Server>,
    summary_lines: &mut Vec<String>,
) -> Result<(), String> {
    let pack = server
        .basic_config
        .get_world_path()
        .join("datapacks")
        .join("function_limit");
    let functions = pack.join("data").join("regress").join("function");
    fs::create_dir_all(&functions).map_err(|err| format!("Failed to create the pack: {err}"))?;
    let files = [
        (
            pack.join("pack.mcmeta"),
            r#"{"pack":{"pack_format":94,"description":""}}"#,
        ),
        (
            functions.join("loop.mcfunction"),
            "scoreboard players add #calls regress 1\nfunction regress:loop\n",
        ),
        (
            functions.join("condition.mcfunction"),
            "scoreboard players add #conditions regress 1\nexecute if function regress:condition run say unreachable\n",
        ),
    ];
    for (path, content) in files {
        fs::write(&path, content)
            .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    }

    let console = |command: &str| {
        server
            .command_dispatcher
            .load()
            .handle_command(&CommandSender::Console.into_source(server), command);
    };
    console("scoreboard objectives add regress dummy");
    // Like vanilla, a new pack folder is unknown until something rescans the folder.
    console("datapack list");
    console("datapack enable \"file/function_limit\"");
    console("function regress:loop");
    console("function regress:condition");

    let world = server.get_world_from_dimension(&Dimension::OVERWORLD);
    let scoreboard = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Each round costs one for the call and one for the scoreboard command; the `function`
    // command itself is free, like vanilla's custom executors.
    let calls = scoreboard.get_score_value("#calls", "regress");
    let limit = server
        .level_info
        .load()
        .game_rules
        .max_command_sequence_length;
    if calls != Some((limit / 2) as i32) {
        return Err(format!(
            "A recursive function ran {calls:?} times, expected {}",
            limit / 2
        ));
    }
    if scoreboard
        .get_score_value("#conditions", "regress")
        .is_none_or(|count| count < 1)
    {
        return Err("A recursive `execute if function` never ran".to_string());
    }
    summary_lines.push("[PASS] bot:recursive_functions_stop_at_the_command_limit".into());
    Ok(())
}
