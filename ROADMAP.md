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
| Worldgen diff | `python tools/parity/worldgen_diff.py --center=x,z --radius r` generates the same seed on both servers (force-loaded square, Pumpkin saving Anvil), then diffs the blocks, grouped by vanilla block -> Pumpkin block with sample positions |
| Upstream sync | Merged `upstream/master` at `2d9de0afb` (2026-10-31, 18 commits) |
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

### RNG lockdown

- `--rng-seed <seed>` on `bot_parity.py` and `parity.py` makes both servers draw gameplay randomness from per-owner Java LCG streams and log every draw. `rng_diff.py` then compares the logs owner by owner and prints the first draw that differs.
- Owners: an entity (`<type>@<block pos where it joined>#<n>`, players `player:<name>`), a scheduled tick (`block@x,y,z`, `fluid@x,y,z`), a chunk (`chunk@cx,cz`: lightning, spawning, random ticks), the chunk list (`chunks`), or `misc` (commands and everything else). Owner `K`'s stream is `LegacyRandomSource(seed ^ K.hashCode() ^ (epoch << 32))`.
- `/tick freeze` restarts every stream and the entity numbering and bumps the epoch, so a scene built while frozen starts from the same state on both servers. Only draws after the last reset are compared. The epoch stops a restarted stream from repeating an existing entity's UUID, which vanilla refuses.
- Vanilla: a `-javaagent` (`tools/parity/rngagent`, java.lang.classfile, built on demand) routes `Entity.random` and `Level.random` and wraps the owner methods in `ServerLevel` and `ServerChunkCache`. It runs on a flat classpath because the bundler's class loader hides Minecraft from the agent.
- Pumpkin: `pumpkin_util::random::gameplay` (`PUMPKIN_RNG_SEED`, `PUMPKIN_RNG_LOG`). Outside the lockdown it is a per-thread `LegacyRand`.
- Since `39a33fb47` all gameplay code in `pumpkin-core` and `pumpkin-inventory` (and the generated enchantment code) draws from `game_random()` instead of `rand`, so every gameplay draw shows up in the log. `GameRandom` has `rand`-shaped `random`, `random_range` and `random_bool` that map to vanilla's draws (`nextFloat`, `nextInt(bound)`, `Mth.nextDouble`, ...), vanilla's `Util.shuffle` and `Util.getRandom` as `shuffle` and `choose`, and `RandomGenerator::Gameplay` for APIs that take a generator (features, XP and int providers). Only networking, keys, telemetry and worldgen still use `rand` or their own seeds.
- Routing a draw doesn't make it match: the method, count, order and arithmetic still have to follow the Java. Matching so far: entity UUIDs, block drops, the item entity constructor, unseeded loot and random ticks (positions, the precipitation roll and which blocks get ticked, wherever the two worlds hold the same blocks). `trace-item-drop` matches vanilla draw for draw; its item trace can still drift once the item lands (#43).
- Code written against `rand`'s traits (`impl Rng` helpers, `ShufflingList`, the gate and sensor brain code) gets a `GameRandom` through a `rand::TryRng` impl. Those draws are logged as `nextInt`/`nextLong` and follow `rand`'s algorithms, so they show up as method mismatches in the diff.
- Random ticks: `Level.getBlockRandomPos` steps a per-owner `randValue` (starting at `LegacyRandomSource(seed ^ ("randValue:" + K).hashCode() ^ (epoch << 32)).nextInt()`) instead of the level's, so positions don't depend on chunk order. Each position is logged under `<owner>/pos` (Pumpkin picks positions before it ticks, so they can't share the owner's sequence), and each block or fluid random tick logs `randomTick` / `fluidRandomTick` with its position and block or fluid. The diff therefore shows whether both servers sample and tick the same blocks.
- `rng_diff.py` counts an owner whose draws match until one log ends as "match until one log ends": the servers keep ticking briefly after a trace, so one usually gets a few more ticks in.

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
| 2 | `execute if/unless block` took a plain block id, so properties (`repeater[delay=1]`) failed to parse. `BlockPredicate::test` ignored properties and NBT, so `/fill ... replace <filter>` did too. Unknown properties and values weren't rejected, and the "no value" error was missing its block argument. | Parity pack | Fixed in `c19fc3e18`; superseded by upstream #4311, whose version was taken in the sync `2d9de0afb` |
| 3 | Codegen mapped 26.3's `IMMOVEABLE` push reaction (the old `BLOCK`) to `PistonBehavior::Ignore`, so pistons could push anvils, grindstones, lodestones, piston heads and the other immovable blocks. Plugins also saw `Ignore` for these blocks. | Update-order tests | Fixed in `55cc36d13` |
| 4 | `pumpkin-codegen` panicked on Windows: template pool ids kept `\` separators, giving invalid identifiers. The panic left the generated files half written. | Running codegen | Fixed in `5ebd5d9fc` |
| 5 | `rustfmt` overflows its 1 MiB main-thread stack on Windows when formatting generated `block.rs`. `format_code` then silently writes the unformatted file, so you get a ~620k-line diff. | Running codegen | Open. Regenerate on Linux or WSL. `55cc36d13` was applied to the formatted file by hand and checked against the codegen output. |
| 6 | Debug `pumpkin-tester` overflowed the Windows main-thread stack when loading a superflat world. The real server was fine. | Parity harness | Worked around in the tester: parity runs on a 64 MiB thread |
| 7 | `/setblock` and `/fill` use `BlockArgumentType`, which has no block states. `setblock ~ ~ ~ repeater[delay=4]` fails to parse, and the target is always the default state. | Reading code | Open |
| 8 | `hopper_fills_chest` and `furnace_smelts_into_hopper` failed on Pumpkin. The hopper and comparator were fine; the comparator had no block entity to hold its output (see #16). | Parity pack | Fixed by #16 |
| 9 | `pumpkin:creeper_should_run_from_cat` fails on vanilla (5/5) and passes about 4/6 on Pumpkin. It only passes in `pumpkin-tester` because of a hand-built arena (`prepare_flat_test_surroundings`). | Parity harness | Open: either the test or Pumpkin's flee AI is wrong |
| 10 | Pumpkin's GameTest runner stores a test's `environment` (game rules, weather, time, difficulty, functions) but doesn't apply it. | Reading code | Open |
| 11 | The `max-chained-neighbor-updates` limit (vanilla's `CollectingNeighborUpdater`) isn't implemented. | Reading code | Open |
| 12 | Gameplay RNG can't be seeded. There's no per-entity or per-world RNG; about 850 call sites use `rand` directly (`rand::rng()`, `rand::random`) or seed a throwaway generator, and random ticks run in parallel. | RNG survey | In progress: the RNG lockdown (`fb5ba55b3`, `2fff476ee`) makes converted sites comparable; see future work |
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
| 24 | `execute if entity @e[tag=...]` used as a condition without `run` fails to parse ("Unknown or incomplete command"). Vanilla answers "Test passed. Count: N". Only `score` conditions could end a command; `entity`, `block`, `blocks`, `loaded`, `dimension`, `biome`, `data` and `stopwatch` couldn't. `if data` also failed with "no elements" instead of counting zero matches. | `drowned/drowned.js` | Fixed in `73364c6bd`; superseded by upstream #4311 (numeric conditionals), whose version was taken in the sync `2d9de0afb`. Not ported: `blocks` still compares block types rather than states and block entities, and has no 32768-block limit |
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
| 39 | Item entities have the same drag issue as #33: vanilla's `ItemEntity.tick` multiplies `getAirDrag()` and the block friction as floats, while `item.rs` uses doubles. | Reading code after #33, `trace-item-drop` | Fixed in `df893357b`; the item trace matches until #43 |
| 40 | `/setblock ... destroy` and `/fill ... destroy` didn't drop the old block's items. Vanilla calls `level.destroyBlock(pos, true)`; Pumpkin passed `SKIP_DROPS`. | `trace-item-drop` | Fixed in `24c888d0a` |
| 41 | Item drops drew their randomness differently from vanilla: no `bobOffs` draw (a client value vanilla's server still draws before the yaw), the motion drawn before the yaw, `mul_add` rounding once where Java rounds twice, and entity UUIDs from the `uuid` crate instead of two `nextLong`s (`Mth.createInsecureUUID`). | RNG lockdown | Fixed in `707eada38` |
| 42 | Scheduled block and fluid ticks run in parallel batches of 32; vanilla runs them one at a time in scheduled order, and redstone can depend on that order. | Reading code (RNG owner wiring) | Open, unverified in-game |
| 43 | An item resting on the ground only moves on ticks where `(tickCount + entityId) % 4 != 0`. Entity IDs differ between the servers, and vanilla's even differ from run to run, so item traces drift once the item lands. | `trace-item-drop` | Harness limitation; would need matching entity IDs |
| 44 | Unseeded loot (block drops, mob deaths, shearing, `/loot`) drew a seed and rolled from its own Xoroshiro. Vanilla's `withOptionalRandomSeed` rolls from the level random when the seed is 0. Block experience and other providers also used fresh Xoroshiros. | Reading code, then the sweep (the seed draw shifted `trace-item-drop` by one draw) | Fixed in `79dbc0cdf` (loot) and `39a33fb47` (providers) |
| 45 | Random tick positions came from `rand::random::<u32>()` with a different bit layout. Vanilla uses `Level.getBlockRandomPos`, a level-wide `randValue * 3 + 1013904223` LCG separate from the level random. | RNG lockdown (`chunk@` draws) | Fixed in `96d446636` |
| 46 | Random ticks ran in parallel batches of 32 samples, so one chunk's samples could run on two threads at once and out of order. Vanilla's `tickChunk` ticks a chunk's samples in order. | Reading code (RNG owner wiring) | Fixed in `0c758ee15` (chunks still tick in parallel) |
| 47 | No precipitation tick: water never froze, snow never piled up, and the per-chunk `nextInt(48)` roll that picks the column was missing. | RNG lockdown (first `chunk@` draw was `nextInt(48)` on vanilla) | Fixed in `f77ef978c`; growing snow layers don't push entities up yet (`Block.pushEntitiesUp`) |
| 48 | Cauldrons never filled from rain or snow (`handlePrecipitation`). | Porting #47 | Fixed in `37d618864` |
| 49 | Natural lightning made an extra `nextInt` instead of using `getBlockRandomPos`, read the heightmap at the chunk corner instead of the chosen column, and struck inside the top block. | Porting #45 | Fixed in `6a740f20a`; lightning rods, entity targeting, `isRainingAt` and local difficulty for the skeleton trap are still TODO |
| 50 | World generation differs near spawn for seed `1792386707805314503`: vanilla has budding amethyst at -77,-31,-3 (Pumpkin: air) and kelp at 12,57,-57 (Pumpkin: none). Other chunks have random-ticking sections on only one server. | Random tick log in `trace-item-drop` | Geodes fixed (#52); kelp height is random tick growth. `worldgen_diff.py` over 81 chunks around spawn is down from 0.22% to 0.016% of blocks; the rest is #56, random tick growth (kelp, amethyst buds) and vanilla's own run-to-run variation (ore blobs, seagrass, see "Worldgen diff" notes) |
| 51 | `/forceload add` only marked chunks as ticking; it never loaded or generated them. Vanilla adds a forced ticket that keeps them loaded and fully generated. | `worldgen_diff.py` (Pumpkin saved no chunks) | Cherry-picked upstream draft #4373 by wsuites as `15ada4d96`; drop it when #4373 lands |
| 52 | Amethyst geodes never generated: the 26.x datapack omits fields equal to the codec defaults, and codegen read the missing `distribution_points` and `point_offset` as 0, so geodes had no shell. Also: the shell noise was seeded from the feature random instead of the world seed (shifting every later draw), the `#minecraft:features_cannot_replace` and `#minecraft:geode_invalid_blocks` tags never resolved (so geodes could replace bedrock and never aborted on water or lava), buds grew into any water instead of a full one, chances were compared as `f32`, and cracks didn't schedule ticks for fluids next to them. | `worldgen_diff.py` | Fixed in `e74774b16` |
| 53 | Chunks that loaded from disk or finished generating with scheduled ticks never ran them: a chunk only joined the scheduled-tick set when something scheduled a new tick in it at runtime. | `worldgen_diff.py` (cave water stayed still) | Fixed in `e2fa21562`, branch only: upstream #4010 (long tick delays) registers chunks on publish the same way, so don't offer this one upstream |
| 54 | Aquifer fluids placed by the noise fill never flowed. Vanilla marks them for post-processing when the aquifer asks for a fluid update; Pumpkin dropped that flag. Now they get a fluid tick, like Pumpkin's carvers already do. | `worldgen_diff.py` | Fixed in `82cf788cb` |
| 55 | Water and lava springs (`spring_feature`) never started flowing: vanilla schedules a fluid tick when it places one. | `worldgen_diff.py` (water missing below a cave spring) | Fixed in `5466dbbd0` |
| 56 | No chunk post-processing. Vanilla's `LevelChunk.postProcessGeneration` ticks marked fluids and runs `updateFromNeighbourShapes` on blocks with `hasPostProcess` (magma, for example, which then grows bubble columns). Pumpkin approximates the fluid half with scheduled ticks (#54) and lacks the block half, so underwater magma has no bubble columns (103 blocks around spawn). | `worldgen_diff.py` | Open; upstream #4022 reworks the bubble column block but not worldgen post-processing |
| 57 | End spikes and the dragon fight's spike list used seed 0 instead of the world seed (vanilla `EndSpikeFeature.getSpikesForLevel`), so every world had the same pillar layout. | Reading code (`end_spike.rs`) | Fixed in `5207044bc`; not checked with the world diff yet (the End isn't enabled in its runs) |

## Future work

### Harness

- **RNG lockdown, next steps** (see "RNG lockdown" above):
  - Make routed draws match, area by area, following `rng_diff.py`: the chunk shuffle (vanilla draws 5168 times under `chunks` in `trace-item-drop`, Pumpkin none), falling blocks (vanilla only), mob AI goals, damage and knockback, then the `rand`-trait helpers listed under "RNG lockdown".
  - Players: vanilla splits a player's draws between `player:<name>` (`ServerPlayer.tick`) and `misc` (`doTick` from the connection); Pumpkin's player tick isn't wrapped yet, so its draws land in `misc`.
  - Entities loaded from disk: vanilla's constructors draw (UUID, `bobOffs`, yaw); Pumpkin's `new_empty` paths don't.
  - Chunk work: Pumpkin only shuffles the spawning chunks when spawning is on, with `rand`'s algorithm.
  - Random ticks in a superflat world, or after fixing #50, to see the ticked blocks match without worldgen noise. Crops and grass could then become step traces.
- **Worldgen diff notes:** vanilla isn't deterministic between runs for features that spill across chunk borders (ore blobs, seagrass), so compare against two vanilla runs before calling a difference a bug. Both servers keep ticking while the world generates, so random tick growth (kelp, amethyst buds) and fluid flow also show up. Next: more seeds and areas, the Nether and End, and #56.
- **More bot checks:** combat, inventories and containers, the "Loading Terrain" case (#4345), and chunk streaming while moving. Run the scaffolding checks against the branch of upstream #3810 to give its reviewers evidence.
- **Update limits.** The 512-deep shape-update limit could be tested. The chained neighbour limit can't be set per test in 26.3, so it needs a different approach.
- **More tests:** blast furnace and smoker speed (26.x `cooking_fuel` speed multiplier), comparators (compare and subtract modes), observers on piston movement, rails and minecarts, sticky piston pull timing, spitting, slime and honey block movement, redstone wire update order, crops and other random ticks (after the RNG lockdown), and entities.
- **Repeat runs** (`--repeat N`, vanilla's `--repeatCount`) to measure flakiness.

### Fixes

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
