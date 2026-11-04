// Undead AI: zombie reinforcements on hard, hurt-by alerts reaching the zombie's subclasses, and
// skeletons fleeing the sun into shade. Mob targets aren't synced, so alerted mobs are told
// apart by the aggressive mob flag their attack goal raises.
const { make } = require('./survival-lib')
const h = make('Undead')
const { bot } = h
const X = 2400; const Y = 200; const Z = 0
const MOB_FLAGS = 15
const AGGRESSIVE = 4
const FLOOR = 16

const mobs = (name) => Object.values(bot.entities).filter(e => e.name === name)
const aggressive = (e) => !!(e && typeof e.metadata[MOB_FLAGS] === 'number' && (e.metadata[MOB_FLAGS] & AGGRESSIVE))
const near = (name, x, z) => mobs(name).find(e => Math.abs(e.position.x - x) < 0.6 && Math.abs(e.position.z - z) < 0.6)

async function clearMobs () {
  h.cmd('tp @e[type=!player] ~ -300 ~')
  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
}

async function summon (type, dx, dz, nbt) {
  const x = X + 0.5 + dx; const z = Z + 0.5 + dz
  h.cmd(`summon ${type} ${x.toFixed(3)} ${Y + 1} ${z.toFixed(3)} {PersistenceRequired:1b,Silent:1b${nbt ? ',' + nbt : ''}}`)
  let e
  await h.waitFor(() => (e = near(type, x, z)), 5000)
  return e
}

// A dark, roofed floor the reinforcement can land anywhere on.
async function darkFloor (r) {
  for (let i = 0; i < 20; i++) {
    h.cmd(`fill ${X - r} ${Y} ${Z - r} ${X + r} ${Y} ${Z + r} stone`)
    h.cmd(`fill ${X - r} ${Y + 4} ${Z - r} ${X + r} ${Y + 4} ${Z + r} stone`)
    if (await h.waitFor(() => h.blockName(X + r, Y + 4, Z + r) === 'stone' && h.blockName(X - r, Y, Z - r) === 'stone', 3000)) break
  }
  await h.ticks(20)
}

// Reinforcements: a caller certain to call, hit on hard. The roofed floor stays within 24
// blocks of the bot, where nothing spawns naturally, so every mob of the kind on it was called.
async function reinforcements (kind, prefix) {
  await darkFloor(FLOOR)
  h.cmd('difficulty hard')
  await clearMobs()
  const spawnedAt = {}
  const listener = (e) => { if (e.name === kind && !spawnedAt[e.id]) spawnedAt[e.id] = e.position.clone() }
  bot.on('entitySpawn', listener)
  const caller = await summon(kind, 0, 2, 'NoAI:1b,Tags:["caller"]')
  h.cmd('attribute @e[tag=caller,limit=1] minecraft:spawn_reinforcements base set 1')
  h.cmd(`item replace entity ${bot.username} hotbar.0 with stick`)
  bot.setQuickBarSlot(0)
  await h.ticks(40)
  const called = () => mobs(kind).filter(e => e.id !== caller.id && Math.abs(e.position.y - (Y + 1)) < 0.5)
  // A call can find no spot on the small floor; the charge only drops when one spawns.
  for (let i = 0; i < 3 && called().length === 0; i++) {
    h.cmd('gamerule spawn_mobs true')
    await h.ticks(2)
    bot.attack(caller)
    await h.ticks(2)
    h.cmd('gamerule spawn_mobs false')
    await h.waitFor(() => called().length > 0, 1500)
  }
  h.result(prefix + '.count', called().length)
  const first = called()[0]
  if (first) {
    const p = spawnedAt[first.id] || first.position
    h.result(prefix + '.block_corner', Number.isInteger(p.x) && Number.isInteger(p.z))
    const d = Math.hypot(p.x - bot.entity.position.x, p.z - bot.entity.position.z)
    h.result(prefix + '.outside_7', d >= 7)
    // Its target is the attacker: it walks over even though the bot is invisible.
    await h.ticks(100)
    const d2 = Math.hypot(first.position.x - bot.entity.position.x, first.position.z - bot.entity.position.z)
    h.result(prefix + '.approaches', d2 < d - 2)
  }
  bot.removeListener('entitySpawn', listener)
  await clearMobs()
  h.cmd('difficulty normal')
  h.cmd(`fill ${X - FLOOR} ${Y + 4} ${Z - FLOOR} ${X + FLOOR} ${Y + 4} ${Z + FLOOR} air`)
  h.cmd(`fill ${X - FLOOR} ${Y} ${Z - FLOOR} ${X + FLOOR} ${Y} ${Z + FLOOR} air`)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('time set midnight')
  h.cmd('weather clear')
  h.cmd('gamerule advance_time false')
  h.cmd('gamerule advance_weather false')
  h.cmd(`effect give ${bot.username} resistance infinite 255 true`)
  h.cmd(`effect give ${bot.username} invisibility infinite 0 true`)
  h.cmd(`effect give ${bot.username} regeneration infinite 255 true`)
  await h.ticks(10)

  await reinforcements('zombie', 'reinforcement')
  await reinforcements('zombified_piglin', 'reinforcement.piglin')
  await h.arena(X, Y, Z, 8)

  // Hurt-by alerts: a zombie wakes its subclasses, but not the zombified piglin it excepts.
  // The others stand beyond the reach an invisible player leaves their own target search.
  const still = 'active_effects:[{id:"minecraft:slowness",amplifier:255,duration:-1,show_particles:0b}]'
  const hurt = await summon('zombie', 0, 2, still)
  const kinds = ['zombie', 'husk', 'drowned', 'zombie_villager', 'zombified_piglin']
  const others = {}
  for (let i = 0; i < kinds.length; i++) {
    others[kinds[i]] = await summon(kinds[i], -6 + i * 3, 7, still)
  }
  await h.ticks(20)
  bot.attack(hurt)
  await h.ticks(40)
  h.result('alert.hurt', aggressive(hurt))
  for (const k of kinds) h.result(`alert.${k}`, aggressive(others[k]))
  await clearMobs()

  // A husk only wakes husks.
  const husk = await summon('husk', 0, 2, still)
  const zombie = await summon('zombie', -3, 7, still)
  const otherHusk = await summon('husk', 3, 7, still)
  await h.ticks(20)
  bot.attack(husk)
  await h.ticks(40)
  h.result('alert.husk_wakes_zombie', aggressive(zombie))
  h.result('alert.husk_wakes_husk', aggressive(otherHusk))
  await clearMobs()

  // A zombified piglin only wakes zombified piglins.
  const piglin = await summon('zombified_piglin', 0, 2, still)
  const otherPiglin = await summon('zombified_piglin', -3, 7, still)
  const piglinZombie = await summon('zombie', 3, 7, still)
  await h.ticks(20)
  bot.attack(piglin)
  await h.ticks(40)
  h.result('alert.piglin_wakes_piglin', aggressive(otherPiglin))
  h.result('alert.piglin_wakes_zombie', aggressive(piglinZombie))
  await clearMobs()

  // Skeletons have no float goal: one dropped into deep water sinks.
  h.cmd(`fill ${X + 4} ${Y + 1} ${Z - 2} ${X + 8} ${Y + 8} ${Z + 2} glass hollow`)
  h.cmd(`fill ${X + 5} ${Y + 2} ${Z - 1} ${X + 7} ${Y + 7} ${Z + 1} water`)
  await h.ticks(20)
  h.cmd(`summon skeleton ${X + 6.5} ${Y + 6} ${Z + 0.5} {PersistenceRequired:1b,Silent:1b}`)
  let sinker
  await h.waitFor(() => (sinker = mobs('skeleton')[0]), 5000)
  h.result('skeleton.in_water', h.blockName(X + 6, Y + 4, Z))
  await h.ticks(200)
  // A floating skeleton bobs at the surface, a sinking one rests on the bottom.
  h.result('skeleton.depth', sinker ? Math.floor(sinker.position.y - Y) : null)
  await clearMobs()
  h.cmd(`fill ${X + 4} ${Y + 1} ${Z - 2} ${X + 8} ${Y + 8} ${Z + 2} air`)

  // Flee sun: a burning skeleton finds the shade under a roof next to it.
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('time set noon')
  h.cmd(`fill ${X + 3} ${Y + 4} ${Z - 8} ${X + 8} ${Y + 4} ${Z + 8} stone`)
  await h.ticks(20)
  const skeleton = await summon('skeleton', -4, 0)
  h.cmd('item replace entity @e[type=skeleton] armor.head with air')
  let shaded = false
  for (let i = 0; i < 30 && !shaded; i++) {
    await h.ticks(10)
    shaded = skeleton.position.x >= X + 3 && skeleton.position.x <= X + 9
  }
  h.result('flee_sun.shaded', shaded)
  await clearMobs()
  h.result('finished', 'ok')
  process.exit(0)
})
