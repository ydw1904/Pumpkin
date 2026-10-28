//! Runs datapack `GameTests` the way vanilla's `net.minecraft.gametest.Main` does, so the same
//! pack can be run on both servers and the JUnit-like reports compared.
//!
//! Usage: `pumpkin-tester parity --packs <dir> [--tests <selector>] [--report <file>]
//! [--world-template <dir>]`. `--world-template` points at the `gametestworld` folder vanilla's
//! runner leaves behind; its `level.dat` and `data/` are copied so Pumpkin generates the same
//! superflat world.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Instant,
};

use pumpkin_core::server::{
    Server,
    builtin_game_tests::builtin_game_test_ids,
    server_test_manager::{
        GameTestBatchReport, GameTestQueueEntry, GameTestRetryOptions,
        prepare_test_run_with_reporter,
    },
};
use pumpkin_data::dimension::Dimension;
use pumpkin_gametest::{GameTestReporter, GameTestRunner};
use pumpkin_util::text::TextComponent;

use crate::harness;

// Vanilla's GameTestServer starts the grid at y=4 and lays out 8 tests per row.
const START_Y: i32 = 4;
const TESTS_PER_ROW: i32 = 8;
// Vanilla packs structures by their size; a fixed generous spacing keeps tests from touching.
const GRID_SPACING: i32 = 32;
const START_X: i32 = 100_000;
const START_Z: i32 = 100_000;
const MAX_TICKS: usize = 20 * 60 * 10;
const PARITY_STACK_SIZE: usize = 64 * 1024 * 1024;

pub struct ParityArgs {
    packs: PathBuf,
    tests: String,
    report: Option<PathBuf>,
    world_template: Option<PathBuf>,
}

impl ParityArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut packs = None;
        let mut tests = "*".to_string();
        let mut report = None;
        let mut world_template = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let mut value = || {
                iter.next()
                    .cloned()
                    .ok_or_else(|| format!("Missing value for {flag}"))
            };
            match flag.as_str() {
                "--packs" => packs = Some(PathBuf::from(value()?)),
                "--tests" => tests = value()?,
                "--report" => report = Some(PathBuf::from(value()?)),
                "--world-template" => world_template = Some(PathBuf::from(value()?)),
                other => return Err(format!("Unknown parity argument '{other}'")),
            }
        }
        Ok(Self {
            packs: packs.ok_or("--packs is required")?,
            tests,
            report,
            world_template,
        })
    }
}

#[derive(Default)]
struct CaptureReporter {
    messages: StdMutex<Vec<String>>,
}

impl CaptureReporter {
    fn snapshot(&self) -> Vec<String> {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl GameTestReporter for CaptureReporter {
    fn send_message(&self, message: TextComponent) {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message.get_text());
    }
}

struct TestRun {
    id: String,
    required: bool,
    report: Arc<GameTestBatchReport>,
    reporter: Arc<CaptureReporter>,
    started: Instant,
}

enum Outcome {
    Passed,
    Failed(String),
}

/// Matches vanilla's test selector wildcards: `*` for any run of characters, `?` for one.
fn selector_matches(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            selector_matches(&pattern[1..], text)
                || (!text.is_empty() && selector_matches(pattern, &text[1..]))
        }
        (Some(b'?'), Some(_)) => selector_matches(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => selector_matches(&pattern[1..], &text[1..]),
        _ => false,
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn prepare_world(world_dir: &Path, args: &ParityArgs) -> Result<(), String> {
    if let Some(template) = &args.world_template {
        fs::copy(template.join("level.dat"), world_dir.join("level.dat"))
            .map_err(|err| format!("Failed to copy level.dat from world template: {err}"))?;
        copy_dir(&template.join("data"), &world_dir.join("data"))
            .map_err(|err| format!("Failed to copy data/ from world template: {err}"))?;
    }
    copy_dir(&args.packs, &world_dir.join("datapacks"))
        .map_err(|err| format!("Failed to copy packs: {err}"))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Writes the same shape as vanilla's `JUnitLikeTestReporter`.
fn write_report(path: &Path, results: &[(TestRun, Outcome, f64)]) -> Result<(), String> {
    let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><testsuite><testsuite>"#);
    for (run, outcome, seconds) in results {
        let id = xml_escape(&run.id);
        let _ = write!(
            xml,
            r#"<testcase classname="{id}" name="{id}" time="{seconds:.3}""#
        );
        match outcome {
            Outcome::Passed => xml.push_str("/>"),
            Outcome::Failed(message) => {
                let element = if run.required { "failure" } else { "skipped" };
                let _ = write!(
                    xml,
                    r#"><{element} message="{}"/></testcase>"#,
                    xml_escape(message)
                );
            }
        }
    }
    xml.push_str("</testsuite></testsuite>");
    fs::write(path, xml).map_err(|err| format!("Failed to write report: {err}"))
}

fn outcome_of(run: &TestRun) -> Outcome {
    let messages = run.reporter.snapshot();
    if !run.report.is_done() {
        return Outcome::Failed(format!("Did not finish within {MAX_TICKS} ticks"));
    }
    if run.report.failed_required() == 0 && run.report.failed_optional() == 0 {
        return Outcome::Passed;
    }
    // The batch summary is also a "failed" line; the reason is the one naming the test.
    let reason = messages
        .iter()
        .rev()
        .find(|m| m.contains(run.id.as_str()) && m.contains("failed"))
        .or_else(|| messages.first())
        .cloned()
        .unwrap_or_else(|| "failed".to_string());
    Outcome::Failed(reason)
}

async fn run_tests(
    server: &Arc<Server>,
    args: &ParityArgs,
) -> Result<Vec<(TestRun, Outcome, f64)>, String> {
    let world = server.get_world_from_dimension(&Dimension::OVERWORLD);
    let selector = args.tests.as_bytes();
    // Pumpkin's Rust function tests share the registry but don't exist on vanilla.
    let builtin = builtin_game_test_ids();
    let ids: Vec<(String, bool)> = server
        .datapack_manager
        .get_test_instance_names()
        .into_iter()
        .filter(|id| selector_matches(selector, id.as_bytes()))
        .filter(|id| !builtin.contains(&id.as_str()))
        .filter_map(|id| {
            let instance = server.datapack_manager.get_test_instance(&id)?;
            (!instance.manual_only).then_some((id, instance.required))
        })
        .collect();

    let mut runner = GameTestRunner::new();
    let mut runs = Vec::with_capacity(ids.len());
    for (slot, (id, required)) in (0i32..).zip(ids) {
        let reporter = Arc::new(CaptureReporter::default());
        let sink: Arc<dyn GameTestReporter> = reporter.clone();
        let report = Arc::new(GameTestBatchReport::new(sink.clone(), 1));
        let entry = GameTestQueueEntry::new(
            id.clone(),
            world.clone(),
            START_X + (slot % TESTS_PER_ROW) * GRID_SPACING,
            START_Z + (slot / TESTS_PER_ROW) * GRID_SPACING,
            0,
            GameTestRetryOptions::no_retries(),
            report.clone(),
        )
        .with_y(START_Y);
        match prepare_test_run_with_reporter(server, entry, sink).await {
            Ok(manager) => runner.enqueue(manager),
            Err(err) => report.fail_to_start(&err),
        }
        runs.push(TestRun {
            id,
            required,
            report,
            reporter,
            started: Instant::now(),
        });
    }

    for _ in 0..MAX_TICKS {
        if runner.is_empty() {
            break;
        }
        server.tick();
        runner.tick().await;
    }
    runner.clear();

    Ok(runs
        .into_iter()
        .map(|run| {
            let outcome = outcome_of(&run);
            let seconds = run.started.elapsed().as_secs_f64();
            (run, outcome, seconds)
        })
        .collect())
}

/// Loading a superflat world template overflows the 1 MiB Windows main-thread stack in debug
/// builds, so the parity run gets its own thread and runtime.
pub fn run_on_large_stack(args: ParityArgs) -> Result<bool, String> {
    std::thread::Builder::new()
        .name("parity".into())
        .stack_size(PARITY_STACK_SIZE)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|err| format!("Failed to build runtime: {err}"))?
                .block_on(run(args))
        })
        .map_err(|err| format!("Failed to spawn parity thread: {err}"))?
        .join()
        .map_err(|_| "Parity thread panicked".to_string())?
}

#[expect(
    clippy::print_stdout,
    reason = "CLI test runner outputs results to stdout"
)]
async fn run(args: ParityArgs) -> Result<bool, String> {
    let temp_dir =
        tempfile::tempdir().map_err(|err| format!("Failed to create world directory: {err}"))?;
    prepare_world(temp_dir.path(), &args)?;
    let server = harness::create_test_server(&temp_dir).await?;

    let results = run_tests(&server, &args).await;
    server.shutdown().await;
    let results = results?;

    let mut failed_required = 0;
    for (run, outcome, _) in &results {
        match outcome {
            Outcome::Passed => println!("PASS {}", run.id),
            Outcome::Failed(message) => {
                if run.required {
                    failed_required += 1;
                }
                println!("FAIL {}: {message}", run.id);
            }
        }
    }
    println!("{} tests, {failed_required} required failed", results.len());
    if let Some(path) = &args.report {
        write_report(path, &results)?;
    }
    Ok(failed_required == 0)
}
