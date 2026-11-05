// Beds and respawning: setting spawn by day, monsters preventing sleep, sleeping through rain,
// respawning at the bed or at world spawn when it is gone,
// keepInventory, respawn anchors charging and exploding in the overworld, and beds exploding /
// anchors setting spawn in the nether.
// pumpkin-settings: allow_nether=true
const { make } = require('./survival-lib')
const h = make('Sleeper')
const { bot, Vec3 } = h
const X = 5200; const Y = 200; const Z = 0

const describe = (j) => j.translate ? j.translate + (j.with ? '(' + j.with.map(w => typeof w === 'object' ? (w.text ?? w.translate) : w).join(',') + ')' : '') : null
const msgs = []
bot.on('message', (j) => { const s = describe(j); if (s && !s.startsWith('commands.') && !s.startsWith('chat.')) msgs.push(s) })
const take = () => { const s = msgs.join('|') || 'none'; msgs.length = 0; return s }
const events = []
bot._client.on('game_state_change', (p) => { const r = String(p.reason); if (!/rain|thunder|game_mode|chunks_load/.test(r)) events.push(r) })
const takeEvents = () => { const s = events.join(',') || 'none'; events.length = 0; return s }
const dimName = () => String(bot.game.dimension).replace('minecraft:', '')
const rel = (p, o) => `${(p.x - o[0]).toFixed(1)},${(p.y - o[1]).toFixed(1)},${(p.z - o[2]).toFixed(1)}`

async function hold (name, n = 1) {
  if (!h.inv()[name]) await h.give(name, n)
  await bot.equip(bot.inventory.items().find(i => i.name === name), 'hand')
}
async function use (x, y, z) {
  await bot.lookAt(new Vec3(x + 0.5, y + 0.5, z + 0.5), true)
  await bot.activateBlock(h.block(x, y, z), new Vec3(0, 1, 0)).catch(e => h.result('use_error', e.message))
  await h.ticks(10)
}
async function placeBed (x, y, z) {
  await hold('white_bed')
  await bot.lookAt(new Vec3(x + 0.5, y + 1, z + 0.5), true)
  await bot.placeBlock(h.block(x, y, z), new Vec3(0, 1, 0)).catch(() => {})
  await h.ticks(5)
  return h.block(x, y + 1, z)
}
async function die () {
  h.cmd(`kill ${bot.username}`)
  await h.respawnAfterDeath()
  await h.waitFor(h.chunksLoaded, 60000)
  await h.ticks(20)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  await h.clear()
  h.cmd('time set 1000')
  h.cmd('weather clear')
  await h.ticks(10)
  const bed = await placeBed(X + 2, Y, Z)
  h.result('bed_placed', bed ? bed.name : 'none')
  take()
  await use(X + 2, Y + 1, Z)
  h.result('day_click', take())

  h.cmd(`summon zombie ${X - 3.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  h.cmd('time set 18000')
  await h.ticks(10)
  await use(X + 2, Y + 1, Z)
  h.result('monster_click', take())
  h.cmd('kill @e[type=zombie]')
  await h.ticks(40)

  // A zombified piglin only stops sleep while it is angry at the player.
  h.cmd(`summon zombified_piglin ${X - 3.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  await h.ticks(10)
  await use(X + 2, Y + 1, Z)
  h.result('calm_piglin_click', take())
  h.result('calm_piglin_sleeping', bot.isSleeping)
  if (bot.isSleeping) {
    await bot.wake().catch(e => h.result('wake_error', e.message))
    await h.waitFor(() => !bot.isSleeping, 5000)
  }
  h.cmd('kill @e[type=zombified_piglin]')
  h.cmd('time set 18000')
  await h.ticks(40)
  take()

  // Only `Monster`s count (an ocelot doesn't), in a box around the head's bottom centre that
  // their own box only has to touch, and never for a creative player.
  const tryBed = async (key) => {
    // A sleep before can have skipped the night.
    h.cmd('time set 18000')
    await h.ticks(10)
    take()
    await use(X + 2, Y + 1, Z)
    h.result(`${key}_click`, take())
    h.result(`${key}_sleeping`, bot.isSleeping)
    if (bot.isSleeping) {
      await bot.wake().catch(e => h.result('wake_error', e.message))
      await h.waitFor(() => !bot.isSleeping, 5000)
      await h.ticks(10)
      take()
    }
  }
  h.cmd(`summon ocelot ${X - 3.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  await h.ticks(10)
  await tryBed('ocelot')
  h.cmd('kill @e[type=ocelot]')
  // Feet 6.9 blocks below the bed, head just inside the box.
  h.cmd(`summon zombie ${X + 2.5} ${Y - 5.9} ${Z + 0.5} {NoAI:1b,NoGravity:1b,PersistenceRequired:1b}`)
  await h.ticks(10)
  await tryBed('low_zombie')
  h.cmd(`gamemode creative ${bot.username}`)
  await h.ticks(10)
  await tryBed('creative_zombie')
  h.cmd(`gamemode survival ${bot.username}`)
  h.cmd('kill @e[type=zombie]')
  await h.ticks(40)
  take()

  h.cmd('weather rain')
  await h.ticks(10)
  await bot.sleep(h.block(X + 2, Y + 1, Z)).catch(e => h.result('sleep_error', e.message))
  await h.ticks(5)
  h.result('sleeping', bot.isSleeping)
  h.result('sleep_msgs', take())
  await h.waitFor(() => !bot.isSleeping, 15000)
  await h.ticks(10)
  h.result('woke_morning', bot.time.timeOfDay < 1000)
  // Waking up clears the weather; the rain fades out over the next 80 ticks.
  await h.ticks(120)
  h.result('rain_after_sleep', bot.isRaining)
  h.cmd('weather clear')

  // Respawn at the bed, then at world spawn once the bed is gone.
  await bot.lookAt(new Vec3(X - 5, Y + 1, Z), true)
  await die()
  h.result('bed_respawn_dim', dimName())
  h.result('bed_respawn_pos', rel(bot.entity.position, [X + 2, Y + 1, Z]))
  h.result('bed_respawn_yaw', bot.entity.yaw.toFixed(2))
  h.result('bed_respawn_events', takeEvents())
  h.cmd(`fill ${X + 2} ${Y + 1} ${Z - 2} ${X + 2} ${Y + 1} ${Z + 2} air`)
  take()
  await die()
  h.result('no_bed_events', takeEvents())
  h.result('no_bed_msgs', take())
  h.result('no_bed_far_from_bed', bot.entity.position.distanceTo(new Vec3(X, Y, Z)) > 100)

  // keepInventory keeps items and levels.
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  h.cmd('gamerule keep_inventory true')
  await h.give('diamond', 3)
  h.cmd(`xp set ${bot.username} 10 levels`)
  await h.waitFor(() => bot.experience.level === 10, 3000)
  await die()
  h.result('keep_inventory', `${h.invStr()} level=${bot.experience.level}`)
  h.cmd('gamerule keep_inventory false')
  await h.clear()

  // Respawn anchor in the overworld: charges up to 4; using it again, with glowstone or an empty
  // hand, blows it up.
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z} respawn_anchor`)
  await h.ticks(5)
  await hold('glowstone', 5)
  for (let i = 0; i < 4; i++) await use(X + 3, Y + 1, Z)
  h.result('anchor_charges', `${h.block(X + 3, Y + 1, Z).getProperties().charges} glowstone_left=${h.inv().glowstone || 0}`)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  take()
  await use(X + 3, Y + 1, Z)
  await h.ticks(5)
  h.result('anchor_full_glowstone_use', h.blockName(X + 3, Y + 1, Z))
  h.cmd(`fill ${X - 8} ${Y + 1} ${Z - 8} ${X + 8} ${Y + 6} ${Z + 8} air`)
  h.cmd(`fill ${X - 8} ${Y} ${Z - 8} ${X + 8} ${Y} ${Z + 8} stone`)
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z} respawn_anchor[charges=1]`)
  await h.clear()
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await h.ticks(5)
  await use(X + 3, Y + 1, Z)
  await h.ticks(5)
  h.result('anchor_empty_hand_use', h.blockName(X + 3, Y + 1, Z))
  h.result('anchor_overworld_msgs', take())
  h.cmd(`fill ${X - 8} ${Y + 1} ${Z - 8} ${X + 8} ${Y + 6} ${Z + 8} air`)
  h.cmd(`fill ${X - 8} ${Y} ${Z - 8} ${X + 8} ${Y} ${Z + 8} stone`)
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z} respawn_anchor`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await h.ticks(5)
  await use(X + 3, Y + 1, Z)
  h.result('anchor_uncharged_use', h.blockName(X + 3, Y + 1, Z))

  // The nether: beds explode, a charged anchor sets spawn and is used up by respawning.
  h.cmd('execute in minecraft:the_nether run forceload add 624 -16 672 16')
  // Forced chunks load in the background, slower on a busy machine; a lamp lights up once they have.
  for (let i = 0; i < 120 && h.blockName(X - 6, Y + 3, Z - 6) !== 'glowstone'; i++) {
    h.cmd(`execute in minecraft:the_nether if loaded 624 64 -16 if loaded 672 64 16 if loaded 624 64 16 if loaded 672 64 -16 if loaded 648 64 0 run execute in minecraft:overworld run setblock ${X - 6} ${Y + 3} ${Z - 6} glowstone`)
    await h.sleep(1000)
  }
  h.cmd(`setblock ${X - 6} ${Y + 3} ${Z - 6} air`)
  h.cmd('execute in minecraft:the_nether run fill 635 99 -5 665 99 5 stone')
  h.cmd('execute in minecraft:the_nether run fill 635 100 -5 665 106 5 air')
  await h.ticks(10)
  h.cmd(`execute in minecraft:the_nether run tp ${bot.username} 655.5 100 0.5`)
  await h.waitFor(() => dimName() === 'the_nether', 20000)
  await h.waitFor(h.chunksLoaded, 60000)
  await h.ticks(20)
  const nbed = await placeBed(658, 99, 0)
  h.result('nether_bed_placed', nbed ? nbed.name : 'none')
  take()
  await use(658, 100, 0)
  await h.ticks(5)
  h.result('nether_bed_after', h.blockName(658, 100, 0))
  h.result('nether_bed_msgs', take())
  h.cmd('execute in minecraft:the_nether run setblock 644 100 0 respawn_anchor[charges=1]')
  h.cmd(`gamemode survival ${bot.username}`)
  h.cmd(`execute in minecraft:the_nether run tp ${bot.username} 646.5 100 0.5`)
  await h.waitFor(() => bot.entity.position.x < 647, 10000)
  await h.clear()
  await h.ticks(5)
  take()
  await use(644, 100, 0)
  h.result('nether_anchor_msgs', take())
  h.result('nether_anchor_state', h.block(644, 100, 0).getProperties().charges)
  await die()
  h.result('anchor_respawn_dim', dimName())
  h.result('anchor_respawn_pos', rel(bot.entity.position, [644, 100, 0]))
  h.result('anchor_after_respawn', h.block(644, 100, 0) ? h.block(644, 100, 0).getProperties().charges : 'unloaded')
  h.result('anchor_respawn_yaw', bot.entity.yaw.toFixed(2))

  // A forced respawn point: an anchor still uses the stand-up search but keeps its charge,
  // water is no place to respawn in, and a missing forced point is forgotten.
  h.cmd('execute in minecraft:the_nether run setblock 644 100 0 respawn_anchor[charges=1]')
  h.cmd(`execute in minecraft:the_nether run spawnpoint ${bot.username} 644 100 0`)
  await h.ticks(5)
  takeEvents()
  await die()
  h.result('forced_anchor_pos', rel(bot.entity.position, [644, 100, 0]))
  h.result('forced_anchor_yaw', bot.entity.yaw.toFixed(2))
  h.result('forced_anchor_charges', h.block(644, 100, 0) ? h.block(644, 100, 0).getProperties().charges : 'unloaded')
  h.cmd('execute in minecraft:the_nether run setblock 650 100 0 water')
  h.cmd(`execute in minecraft:the_nether run spawnpoint ${bot.username} 650 100 0`)
  await h.ticks(5)
  takeEvents()
  await die()
  h.result('forced_water_dim', dimName())
  h.result('forced_water_events', takeEvents())
  await die()
  h.result('forced_forgotten_events', takeEvents())
  h.cmd('execute in minecraft:the_nether run forceload remove all')
})
