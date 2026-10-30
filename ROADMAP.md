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
| Parity pack | 29 generated tests in `tools/parity/tests.py`; all 29 pass on vanilla and Pumpkin |
| Bot checks | `python tools/parity/bot_parity.py <check>` runs a headless 26.3 Mineflayer bot on vanilla and Pumpkin and diffs what it saw. `smoke`, `falling/landing-sand.js`, `scaffolding/landing-overhang.js`, `scaffolding/landing-pane-fence.js` and `beacon/stale-heightmap.js` match fully; `activator/activator.js` (#31) matches apart from #32; the remaining differences are #25 to #30 |
| Pumpkin's own suite | `cargo run -p pumpkin-tester`: all bot tests and built-in GameTests pass |

### How the harness works

- Vanilla runs through its built-in headless runner: `java -DbundlerMainClass=net.minecraft.gametest.Main -jar server.jar --packs <dir> --tests <selector> --report <xml>`.
- Pumpkin runs through `pumpkin-tester parity`. It takes the same arguments and writes the same JUnit-like XML.
- Both servers use vanilla's superflat test world: the vanilla run leaves a `gametestworld` behind, and `pumpkin-tester` copies its `level.dat` and `data/` into Pumpkin's world.
- `tools/parity/build_pack.py` turns the specs in `tests.py` into structures, functions and test instances. Each test is a start test block plus a mechanic. It passes either:
  - directly, when the mechanic powers an accept block, or
  - through a timed check, where a repeater line fires a command block that runs `execute if block` checks.
- The vanilla jar, run logs and decompiled sources live outside the repo, in `~/pumpkin-parity`.

### Bot checks

- `tools/parity/bot_parity.py <check>` boots a vanilla server (offline mode, whitelist off) and a Pumpkin server (online mode, encryption, telemetry and Bedrock off) one after the other, with the same seed.
- For each server it runs `tools/parity/bots/<check>.js` and compares the `RESULT <key> <value>` lines the check prints.
- Checks can send console commands by appending lines to the file in `$CMDS`.
- ydw's `pumpkin-checks` run unchanged: pass a path such as `falling/landing-sand.js`. Only the observed value of each `PASS/FAIL <label>: got <value>` and `INFO <label>: <value>` line is compared, so vanilla stays the reference.
- `--release` uses a release build of Pumpkin. Use it for timing.
- Mineflayer comes from ydw's private 26.3 workspace (`ydw1904/mineflayer-26.3`). Clone it into `~/pumpkin-parity` with `gh repo clone` (plain `git` uses an account without access), then run `npm ci --ignore-scripts` and `npm run prepare -w node-minecraft-data`.
- On Windows, replace `node-minecraft-data/minecraft-data` with a directory junction to `../minecraft-data`. Git checks the symlink out as a text file.
- Avoid comparing values that vanilla randomises itself, such as the exact spawn point. Take readings before something else changes them; for example, measure fall damage from the first health update, before natural regeneration heals it.

### Step traces

- A check built on `tools/parity/bots/steptrace.js` freezes the game (`/tick freeze`), builds a scene, then runs `/tick step 1` at a time and reads a fixed set of probes after each step. It prints `t<tick>.<probe>` results; `bot_parity.py` reports the first tick where the servers disagree. `--diffs-only` hides matching lines.
- Probes are command replies (normalised so number formatting doesn't count), the bot's own view of a block (what a client was sent), or the server's block state through `execute if block` against a list of candidate states.
- A step is done once `/time query gametime` has advanced and two probe passes agree. Pumpkin runs chat commands as async tasks next to the tick and advances game time first, so a single read can land mid-tick.
- Randomness still differs, so traces only line up for deterministic mechanics until the RNG lockdown. Player movement isn't frozen (it's client authoritative).
- Pumpkin's `/setblock` can't take block states yet (#7), so scenes use default states.
- `trace-falling-sand` and `trace-piston` match on the server side; the piston's client view differs (#35, #36).

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
| 8 | `hopper_fills_chest` and `furnace_smelts_into_hopper` failed on Pumpkin. The hopper and comparator were fine; the comparator had no block entity to hold its output (see #16). | Parity pack | Fixed by #16 |
| 9 | `pumpkin:creeper_should_run_from_cat` fails on vanilla (5/5) and passes about 4/6 on Pumpkin. It only passes in `pumpkin-tester` because of a hand-built arena (`prepare_flat_test_surroundings`). | Parity harness | Open: either the test or Pumpkin's flee AI is wrong |
| 10 | Pumpkin's GameTest runner stores a test's `environment` (game rules, weather, time, difficulty, functions) but doesn't apply it. | Reading code | Open |
| 11 | The `max-chained-neighbor-updates` limit (vanilla's `CollectingNeighborUpdater`) isn't implemented. | Reading code | Open |
| 12 | Gameplay RNG can't be seeded. There's no per-entity or per-world RNG; about 500 call sites use the thread-local `rand::rng()`, and random ticks run in parallel. | RNG survey | Open, see future work |
| 13 | Pumpkin reads structure palettes with `Name`/`Properties` even at data version 5023, where vanilla places air. | Building the pack | Won't fix for now: more lenient than vanilla, nothing breaks |
| 14 | `execute if block` doesn't reject unloaded positions (vanilla uses `getLoadedBlockPos`). | Reading code | Open, unverified. The terminal-condition part is fixed by #24 |
| 15 | Every scheduled block and fluid tick ran one tick late. `ChunkTickScheduler::step_tick` advanced its offset after taking the current slot, so ticks scheduled during that tick landed at `delay + 1`. A delay-4 repeater took 9 game ticks instead of 8. This affected all redstone, fluid and falling-block timing. | Timing probes (`8k + 2` on vanilla, `9k + 2` on Pumpkin) | Fixed in `36222e976`, with unit tests that fail on the old code and two furnace race tests |
| 16 | GameTest structure placement only created a block entity when the template had NBT for it. Vanilla's `setBlock` always creates one, so a comparator from a template had nowhere to store its output and never powered anything. | Parity pack | Fixed in `d7223a56e` |
| 17 | Pumpkin's GameTest runner reports test ticks one lower than vanilla (`8k + 1` against `8k + 2` with identical in-world timing). It also starts a test one tick after placing it, where vanilla's `startExecution(1)` gives two. | Timing probes | Open, low priority: it only affects reported ticks and `max_ticks` margins |
| 18 | Natural world-generation structures may have the same gap as #16 (block-entity blocks placed from templates without NBT). | Reading code | To check |
| 19 | Fall damage came up short. `LivingEntity::fall` only added a move's drop when the move didn't end on the ground, so the move that lands was never counted. Vanilla's `Entity.checkFallDamage` adds it first. A 10 block drop dealt 6 damage instead of 7, for players and mobs. | Smoke bot | Fixed in `667bb777a` |
| 20 | After a ~215 block teleport, Pumpkin takes longer to send the chunks around the destination: 2.4 s on a release build against 0.8 to 1.0 s on vanilla (11 s on a debug build). Profiled with temporary timing logs: sending takes ~100 ms once a chunk exists, so it's all generation. The whole view area (193 chunks) finishes at once, and the player's chunk is ready only when ~95% of the area is. The cause is the features order gate from upstream `edfbb4892` (`GenerationSchedule::is_features_order_ready`): a chunk only places features after every chunk within 2 to its west, or in the same column to its north, has, so the center waits on a chain reaching the western edge of the area. With the gate switched off, the center was sent after 349 ms and the bot saw chunks after 403 ms (vanilla 829 ms). Ranking tasks by distance before stage and position (`TaskPriority` field order) only saved ~0.3 s. | Smoke bot, timing logs | Open, design trade-off: the gate keeps feature placement deterministic across chunk borders, and any fixed global order has this chain. Raise with the maintainers rather than change it on the branch. Probably relevant to #4345 ("Loading Terrain") |
| 21 | Pumpkin keeps `fall_distance` as `f32`; vanilla 26.3 uses a `double` (adding `float` deltas). This can flip the floor in fall damage at exact block boundaries. | Reading code | Open, low priority |
| 22 | Using a snow item on a snow layer's top face didn't add a layer. `LayeredSnowBlock::can_update_at` only accepted `Down`, which is the neighbour case; clicking the snow itself passes `Up`. | `falling/landing-sand.js` | Fixed in `e40068855` |
| 23 | An entity whose vehicle was removed (e.g. `/kill`) still counts as riding it, and a pig can't mount a new minecart afterwards. Vanilla's `Entity.setRemoved` makes a destroyed entity stop riding and drops its passengers, and `LivingEntity.dismountVehicle` teleports a living rider to its own position (raised to the vehicle's height, moved out of blocks). | `dismount/dismount.js` | Fixed in `46d51aa10`; A and B match vanilla. Not ported: unload and dimension-change removals (they don't go through `World::remove_entity`), and the plugin dismount events, which the existing `remove_passenger_sync` callers don't fire either. The remaining check difference is #30 |
| 24 | `execute if entity @e[tag=...]` used as a condition without `run` fails to parse ("Unknown or incomplete command"). Vanilla answers "Test passed. Count: N". Only `score` conditions could end a command; `entity`, `block`, `blocks`, `loaded`, `dimension`, `biome`, `data` and `stopwatch` couldn't. `if data` also failed with "no elements" instead of counting zero matches. | `drowned/drowned.js` | Fixed in `73364c6bd`, branch only: upstream draft #4378 (opened 2026-10-28) makes the same change, so don't offer this one upstream; drop it when #4378 lands. Not ported: `blocks` still compares block types rather than states and block entities, and has no 32768-block limit |
| 25 | Zombies don't turn into drowned under water, and husks don't turn into zombies. Neither the shaking flag nor the conversion sounds appear. | `drowned/drowned.js`, `drowned/gear.js` | Open; ydw's checks cover it, and upstream #4039 covers drowned swimming AI. Check with ydw before starting |
| 26 | `/summon` with equipment NBT still rolls random spawn gear on top, skeletons get a bow over the NBT sword, and `CanPickUpLoot:1b` is lost. | `summon/summon-nbt.js`, `summon-gear/summon-gear.js` | Open; upstream #4391 ("roll spawn equipment only when a mob is finalized") looks like it covers it |
| 27 | Beacons don't open their window, apply effects or play their sounds. | `beacon/beacon.js` | Covered by ydw's upstream #3860 |
| 28 | A wither killed with `mob_drops false` still drops its nether star. | `wither/wither-star.js` | Probably covered by ydw's upstream #3865 |
| 29 | Scaffolding placement (stacking, extending, sneak placement, falling at distance 7, waterlogging, the top layer) is mostly missing. | `scaffolding/*` | Covered by upstream #3810 |
| 30 | A riding player's server-side position doesn't follow the vehicle. After a dismount, vanilla puts the player at the vehicle (x 1.5 in `dismount.js`) while Pumpkin still has the spot where they mounted (x 0.5). The check's last step ("server sees the bot walk") fails on vanilla too, so only the position is a real difference. | `dismount/dismount.js` | Open. Upstream draft #4378 adds `position_riders` but leaves players client authoritative |
| 31 | A ridden minecart on a powered activator rail froze the server for good. The activator-rail branch in `minecart.rs` held the `passengers` lock and called `remove_passenger_sync` on the same minecart, which locks it again, so the tick thread deadlocked. Commands stopped running after that point. | Reading code, `activator/activator.js` | Fixed in `0bce7341c`: the minecart ejects its passengers last first through `remove_passenger`, like vanilla's `ejectPassengers` and `stopRiding`, so plugin dismount events fire and the passenger is moved off the cart. The check's pig and player cases match vanilla apart from where the player lands (#32) |
| 32 | A player leaving a minecart lands on a different side than on vanilla (`activator.js`: vanilla north of the cart, Pumpkin west). Vanilla's `AbstractMinecart.getDismountLocationForPassenger` orders the candidate spots by the cart's motion direction (`getMotionDirection`); Pumpkin's `remove_passenger` uses the vehicle's yaw for every vehicle. Affects sneak dismounts too. | `activator/activator.js` | Open |
| 33 | Falling blocks drifted from vanilla's trajectory by ~1e-7 per tick. Vanilla scales velocity by `getAirDrag()`, a float `0.98F` (`0.9800000190734863` as a double); Pumpkin used the double `0.98`. | `trace-falling-sand` | Fixed in `74f6936c4`; the 20-tick trace now matches vanilla bit for bit |
| 34 | `/tick freeze` still ran block events, so a piston powered while frozen moved before any `/tick step`. Vanilla's `ServerLevel.tick` only calls `runBlockEvents` when the game runs normally. | `trace-piston` | Fixed in `bfed0d9d8`; the server-side piston trace matches vanilla. Upstream draft #3057 (piston rewrite, last updated 2026-08) also drops the call |
| 35 | Pumpkin sends `moving_piston` block updates to clients while a piston moves. Vanilla places them with flags 68 (no client update) and lets the client animate from the block event, so clients only see the final blocks. | `trace-piston` (client view) | Open, low priority. Check whether it causes flicker on a real client |
| 36 | `/data get block` on a moving piston returns no data on Pumpkin. Vanilla shows `blockState`, `facing`, `progress`, `extending` and `source`. | `trace-piston` | Open, unverified whether the block entity writes no NBT or the command drops it |
| 37 | A falling block's name is "Falling Block" on Pumpkin; vanilla names it after the block ("Falling Sand"), which shows in command feedback. | `trace-falling-sand` | Open |
| 38 | SNBT doubles print without a fraction on Pumpkin (`0d` against vanilla's `0.0d`), so `/data get` output differs. Step traces normalise numbers so this doesn't hide real differences. | `trace-falling-sand` | Open |
| 39 | Item entities have the same drag issue as #33: vanilla's `ItemEntity.tick` multiplies `getAirDrag()` and the block friction as floats, while `item.rs` uses doubles. | Reading code after #33 | Open, unverified by a trace |

## Future work

### Harness

- **RNG lockdown (next),** so that results depending on randomness are comparable:
  - Post-hook every RNG call on both servers and return values from isolated Java-LCG streams, keyed per owner (entity index in the test, block position, level).
  - Vanilla side: a `-javaagent` using the JDK ClassFile API. Pumpkin side: a `pumpkin_util` wrapper in place of `rand::rng()`, with a thread-local current owner.
  - Log every draw on both sides and diff the logs to find the first divergent call.
- **More bot checks:** combat, inventories and containers, the "Loading Terrain" case (#4345), and chunk streaming while moving. Run the scaffolding checks against the branch of upstream #3810 to give its reviewers evidence.
- **Update limits.** The 512-deep shape-update limit could be tested. The chained neighbour limit can't be set per test in 26.3, so it needs a different approach.
- **More tests:** blast furnace and smoker speed (26.x `cooking_fuel` speed multiplier), comparators (compare and subtract modes), observers on piston movement, rails and minecarts, sticky piston pull timing, spitting, slime and honey block movement, redstone wire update order, crops and other random ticks (after the RNG lockdown), and entities.
- **Repeat runs** (`--repeat N`, vanilla's `--repeatCount`) to measure flakiness.

### Fixes

- Port #33's float drag to items (#39) and check it with a trace.
- Investigate #9 (the creeper test), #18 (block entities in generated structures) and #20 (teleport chunk delay).
- Coordinate with upstream #4010 before offering the scheduler fix (#15): both change `tick/scheduler.rs`.
- Block states in `/setblock` and `/fill` (#7).
- Apply test environments in the GameTest runner (#10).
- Per-entity and per-world RNG matching vanilla (#12). This is a larger refactor, separate from the harness hooks.
- Open crash issues upstream with the `panic` label, especially ones that affect normal play:
  - #4345: infinite "Loading Terrain" screen.
  - #4176: Nether generation deadlock.
  - #4352: `/fill` destroy deadlock.
  - #3378: creeper explosion disconnects.
  - #3844: crossbow kick.
