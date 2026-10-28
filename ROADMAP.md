# sean/playable: branch state and roadmap

This branch is for getting Pumpkin into a playable state and checking it against vanilla. It's
based on `Pumpkin-MC/Pumpkin` master (`09e63cb8d`) and is kept separate from the upstream
contribution work in this fork. Fix commits are self-contained so a human can offer them upstream
separately. Agents never open upstream PRs; see `AGENTS.md`.

## Branch state

| Area | State |
| :--- | :---- |
| Target version | Java 26.3 (protocol 777), vanilla server needs JDK 25 |
| Parity harness | `python tools/parity/parity.py` runs the same datapack GameTests on vanilla and Pumpkin and diffs the results |
| Parity pack | 27 generated tests in `tools/parity/tests.py`; all 27 pass on vanilla, 25 on Pumpkin |
| Pumpkin's own suite | `cargo run -p pumpkin-tester`: all bot tests and built-in GameTests pass |

### How the harness works

- Vanilla runs through its built-in headless runner: `java -DbundlerMainClass=net.minecraft.gametest.Main -jar server.jar --packs <dir> --tests <selector> --report <xml>`.
- Pumpkin runs through `pumpkin-tester parity`. It takes the same arguments and writes the same JUnit-like XML.
- Both servers use vanilla's superflat test world: the vanilla run leaves a `gametestworld` behind, and `pumpkin-tester` copies its `level.dat` and `data/` into Pumpkin's world.
- `tools/parity/build_pack.py` turns the specs in `tests.py` into structures, functions and test instances. Each test is a start test block plus a mechanic. It passes either:
  - directly, when the mechanic powers an accept block, or
  - through a timed check, where a repeater line fires a command block that runs `execute if block` checks.
- The vanilla jar, run logs and decompiled sources live outside the repo, in `~/pumpkin-parity`.

### Status values

- `ok`: both servers pass.
- `PARITY`: passes on vanilla, fails on Pumpkin. This is a Pumpkin bug.
- `PUMPKIN-ONLY PASS`: Pumpkin passes something vanilla doesn't.
- `both fail`: the test itself is probably wrong.

### Setup notes

- **Windows prerequisites:** rustup, Visual Studio Build Tools (C++ workload, for the MSVC linker) and Temurin JDK 25.
- **Test selectors need a namespace,** e.g. `pumpkin_parity:*`. On Windows, `java.exe` expands a bare `*` against files in the working directory.
- **Datapack structure palettes use `id`/`properties`.** These replaced `Name`/`Properties` at data version 5006, and vanilla places air if a current-version structure uses the old names.
- **Telemetry is on by default.** Pumpkin sends anonymous heartbeats to `market.pumpkinmc.org`; turn it off with the `telemetry` setting.

## Bugs found

| # | Bug | Found by | Status |
| -: | :-- | :------- | :----- |
| 1 | The start test block was a 1-tick pulse. Vanilla's `TestBlockEntity.trigger` never schedules its reset, so it stays powered and every block-based test started differently. | Reading vanilla source | Fixed in `2776ca07b` |
| 2 | `execute if/unless block` took a plain block id, so properties (`repeater[delay=1]`) failed to parse. `BlockPredicate::test` ignored properties and NBT, so `/fill ... replace <filter>` did too. Unknown properties and values weren't rejected, and the "no value" error was missing its block argument. | Parity pack | Fixed in `c19fc3e18` |
| 3 | Codegen mapped 26.3's `IMMOVEABLE` push reaction (the old `BLOCK`) to `PistonBehavior::Ignore`, so pistons could push anvils, grindstones, lodestones, piston heads and the other immovable blocks. Plugins also saw `Ignore` for these blocks. | Update-order tests | Fixed in `55cc36d13` |
| 4 | `pumpkin-codegen` panicked on Windows: template pool ids kept `\` separators, giving invalid identifiers. The panic left the generated files half written. | Running codegen | Fixed in `5ebd5d9fc` |
| 5 | `rustfmt` overflows its 1 MiB main-thread stack on Windows when formatting generated `block.rs`. `format_code` then silently writes the unformatted file, so you get a ~620k-line diff. | Running codegen | Open. Regenerate on Linux or WSL. `55cc36d13` was applied to the formatted file by hand and checked against the codegen output. |
| 6 | Debug `pumpkin-tester` overflowed the Windows main-thread stack when loading a superflat world. The real server was fine. | Parity harness | Worked around in the tester: parity runs on a 64 MiB thread |
| 7 | `/setblock` and `/fill` use `BlockArgumentType`, which has no block states. `setblock ~ ~ ~ repeater[delay=4]` fails to parse, and the target is always the default state. | Reading code | Open |
| 8 | `hopper_fills_chest` and `furnace_smelts_into_hopper` fail on Pumpkin. Either the hopper transfer or the comparator reading the container differs. | Parity pack | Open, not investigated |
| 9 | `pumpkin:creeper_should_run_from_cat` fails on vanilla (5/5) and passes about 4/6 on Pumpkin. It only passes in `pumpkin-tester` because of a hand-built arena (`prepare_flat_test_surroundings`). | Parity harness | Open: either the test or Pumpkin's flee AI is wrong |
| 10 | Pumpkin's GameTest runner stores a test's `environment` (game rules, weather, time, difficulty, functions) but doesn't apply it. | Reading code | Open |
| 11 | The `max-chained-neighbor-updates` limit (vanilla's `CollectingNeighborUpdater`) isn't implemented. | Reading code | Open |
| 12 | Gameplay RNG can't be seeded. There's no per-entity or per-world RNG; about 500 call sites use the thread-local `rand::rng()`, and random ticks run in parallel. | RNG survey | Open, see future work |
| 13 | Pumpkin reads structure palettes with `Name`/`Properties` even at data version 5023, where vanilla places air. | Building the pack | Won't fix for now: more lenient than vanilla, nothing breaks |
| 14 | `execute if block` doesn't reject unloaded positions (vanilla uses `getLoadedBlockPos`). I also haven't checked whether `if/unless block` work as terminal conditions without `run`. | Reading code | Open, unverified |

## Future work

### Harness

- **RNG lockdown,** so that results depending on randomness are comparable:
  - Post-hook every RNG call on both servers and return values from isolated Java-LCG streams, keyed per owner (entity index in the test, block position, level).
  - Vanilla side: a `-javaagent` using the JDK ClassFile API. Pumpkin side: a `pumpkin_util` wrapper in place of `rand::rng()`, with a thread-local current owner.
  - Log every draw on both sides and diff the logs to find the first divergent call.
- **Bot layer.** Mineflayer stops at 26.1, but Pumpkin's tester bot speaks 26.3 and can log into vanilla too. That needs `online-mode=false`, `network-compression-threshold=-1` and keep-alive replies. It would cover login, chunk loading, movement and combat.
- **Update limits.** The 512-deep shape-update limit could be tested. The chained neighbour limit can't be set per test in 26.3, so it needs a different approach.
- **More tests:** comparators (compare and subtract modes), observers on piston movement, rails and minecarts, sticky piston pull timing, spitting, slime and honey block movement, redstone wire update order, crops and other random ticks (after the RNG lockdown), and entities.
- **Repeat runs** (`--repeat N`, vanilla's `--repeatCount`) to measure flakiness.

### Fixes

- Investigate #8 (hopper/comparator) and #9 (creeper test).
- Block states in `/setblock` and `/fill` (#7).
- Apply test environments in the GameTest runner (#10).
- Per-entity and per-world RNG matching vanilla (#12). This is a larger refactor, separate from the harness hooks.
- Open crash issues upstream with the `panic` label, especially ones that affect normal play:
  - #4345: infinite "Loading Terrain" screen.
  - #4176: Nether generation deadlock.
  - #4352: `/fill` destroy deadlock.
  - #3378: creeper explosion disconnects.
  - #3844: crossbow kick.
