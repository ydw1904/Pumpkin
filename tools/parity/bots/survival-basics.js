// Everyday survival mechanics: food exhaustion from every source, starvation and peaceful
// regeneration, drowning, suffocation, fire, lava, powder snow, magma, berry bushes, cactus, the
// void, status effect ticks and reapplying effects, beds and time_since_rest. Values come from
// `/data get entity` replies, health updates and the statistics screen.
const { make } = require('./survival-lib')
const h = make('Basics', 900000)
const { bot, Vec3 } = h
const X = 12400; const Y = 200; const Z = 0
const r3 = (n) => Math.round(n * 1000) / 1000
const r2 = (n) => Math.round(n * 100) / 100

const chat = []
bot.on('message', (m) => chat.push(m.toString()))
const translated = []
bot.on('message', (j) => { if (j.translate) translated.push(j.translate) })
const takeTranslated = () => { const s = translated.filter(t => !t.startsWith('commands.') && !t.startsWith('chat.')).join('|') || 'none'; translated.length = 0; return s }

// The value of a `/data get entity @s <path>` reply, or null.
async function nbt (path) {
  const c0 = chat.length
  const reply = () => chat.slice(c0).find((m) => m.includes('entity data') || m.includes('Found no elements') || m.includes('No entity'))
  bot.chat(`/data get entity @s ${path}`)
  if (!await h.waitFor(() => reply() !== undefined, 5000)) return null
  const text = reply()
  if (!text.includes('entity data')) return null
  return text.replace(/^.*entity data: /, '')
}
async function num (path) {
  const v = await nbt(path)
  if (v === null) return NaN
  const m = v.match(/-?\d+(\.\d+)?(E-?\d+)?/)
  return m ? Number(m[0]) : NaN
}
const food = async () => ({ ex: await num('foodExhaustionLevel'), sat: await num('foodSaturationLevel'), food: await num('foodLevel') })
// Exhaustion added between two readings; every 4 points taken off show up as a point of
// saturation (or food) lost.
const exDelta = (a, b) => r3(b.ex - a.ex + 4 * ((a.sat - b.sat) + (a.food - b.food)))

// Vanilla `BuiltInRegistries.STAT_TYPE` order; custom stat ids follow custom_stats.json.
const MINED = 0; const CUSTOM = 8
const CUSTOM_STATS = ['leave_game', 'play_time', 'total_world_time', 'time_since_death', 'time_since_rest',
  'sneak_time', 'walk_one_cm', 'crouch_one_cm', 'sprint_one_cm', 'walk_on_water_one_cm', 'fall_one_cm',
  'climb_one_cm', 'fly_one_cm', 'walk_under_water_one_cm', 'minecart_one_cm', 'boat_one_cm', 'pig_one_cm',
  'happy_ghast_one_cm', 'horse_one_cm', 'aviate_one_cm', 'swim_one_cm', 'strider_one_cm', 'nautilus_one_cm',
  'jump', 'drop', 'damage_dealt', 'damage_dealt_absorbed', 'damage_dealt_resisted', 'damage_blocked_by_shield',
  'damage_taken', 'damage_absorbed', 'damage_resisted', 'deaths', 'mob_kills', 'animals_bred', 'player_kills',
  'fish_caught', 'talked_to_villager', 'traded_with_villager', 'eat_cake_slice', 'fill_cauldron',
  'use_cauldron', 'clean_armor', 'clean_banner', 'clean_shulker_box', 'interact_with_brewingstand',
  'interact_with_beacon', 'inspect_dropper', 'inspect_hopper', 'inspect_dispenser', 'play_noteblock',
  'tune_noteblock', 'pot_flower', 'trigger_trapped_chest', 'open_enderchest', 'enchant_item', 'play_record',
  'interact_with_furnace', 'interact_with_crafting_table', 'open_chest', 'sleep_in_bed', 'open_shulker_box']
const stats = {}
async function readStats () {
  const reply = new Promise((resolve) => {
    bot._client.once('statistics', (p) => resolve(p.entries))
    setTimeout(() => resolve([]), 5000)
  })
  bot._client.write('client_command', { actionId: 'request_stats' })
  // Vanilla only sends the stats that changed since the last request.
  for (const e of await reply) {
    if (e.categoryId === CUSTOM && CUSTOM_STATS[e.statisticId]) stats[CUSTOM_STATS[e.statisticId]] = e.value
    if (e.categoryId === MINED) stats[`mined:${e.statisticId}`] = e.value
  }
  return { ...stats }
}
const statDelta = (a, b, name) => (b[name] || 0) - (a[name] || 0)

// Health updates in arrival order.
const healths = []
bot.on('health', () => healths.push(r2(bot.health)))
async function hurtsDuring (ticks) {
  const h0 = healths.length; const start = bot.health
  await h.ticks(ticks)
  const seen = healths.slice(h0)
  const drops = seen.filter((v, i) => v < (i === 0 ? start : seen[i - 1])).length
  return `drops=${drops} health=${r2(bot.health)}`
}

async function resetPlayer () {
  h.cmd(`effect clear ${bot.username}`)
  // Instant effects take their duration in ticks and run out on their own.
  h.cmd(`effect give ${bot.username} instant_health 1 10`)
  h.cmd(`effect give ${bot.username} saturation 2 10`)
  await h.waitFor(() => bot.health >= 20 && bot.food >= 20, 5000)
  await h.ticks(25)
}
async function setHealth (hp) {
  await resetPlayer()
  if (hp < 20) {
    h.cmd(`damage ${bot.username} ${20 - hp} minecraft:generic_kill`)
    await h.waitFor(() => bot.health < 20, 3000)
  }
  await h.ticks(2)
}
const stand = async (x = X, z = Z) => {
  h.cmd(`tp ${bot.username} ${x + 0.5} ${Y + 1} ${z + 0.5} 0 0`)
  await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(x + 0.5, Y + 1, z + 0.5)) < 0.3, 5000)
  await h.waitFor(() => bot.entity.onGround, 3000)
  await h.ticks(5)
}
const clearArena = async () => {
  h.cmd(`fill ${X - 12} ${Y + 1} ${Z - 12} ${X + 12} ${Y + 6} ${Z + 12} air`)
  h.cmd(`fill ${X - 12} ${Y} ${Z - 12} ${X + 12} ${Y} ${Z + 12} stone`)
  h.cmd('kill @e[type=!player]')
  await h.ticks(10)
}
const controls = async (keys, ticks) => {
  for (const k of keys) bot.setControlState(k, true)
  await h.ticks(ticks)
  bot.clearControlStates()
  await h.waitFor(() => bot.entity.onGround, 3000)
  await h.ticks(10)
}

async function exhaustionScenes () {
  h.cmd('gamerule natural_health_regeneration false')
  await resetPlayer()
  const measure = async (key, action) => {
    await stand()
    const a = await food(); const s0 = await readStats()
    await action()
    const b = await food(); const s1 = await readStats()
    h.result(`ex.${key}`, `${exDelta(a, b)} jump+${statDelta(s0, s1, 'jump')}`)
  }
  await measure('idle', () => h.ticks(20))
  await measure('jump', () => controls(['jump'], 2))
  await measure('walk20', () => controls(['forward'], 20))
  await measure('sneak20', () => controls(['forward', 'sneak'], 20))
  // Sprinting distance on the client varies by a tick between runs, so sprinting is read as
  // exhaustion per metre of the sprint stat.
  await stand()
  {
    const a = await food(); const s0 = await readStats()
    await controls(['forward', 'sprint'], 20)
    const b = await food(); const s1 = await readStats()
    const metres = statDelta(s0, s1, 'sprint_one_cm') / 100
    h.result('ex.sprint_per_metre', metres > 0 ? Math.round(exDelta(a, b) / metres * 100) / 100 : 'no sprint')
    const c = await food()
    await stand()
    await controls(['forward', 'sprint', 'jump'], 3)
    const d = await food()
    h.result('ex.sprint_jump_has_0.2', exDelta(c, d) >= 0.2 && exDelta(c, d) < 0.3)
  }

  // Damage: each type has its own exhaustion, only when health is lost.
  for (const type of ['generic', 'mob_attack', 'in_fire', 'cactus', 'magic', 'starve', 'drown', 'hot_floor']) {
    await resetPlayer()
    const a = await food()
    h.cmd(`damage ${bot.username} 2 minecraft:${type}`)
    await h.waitFor(() => bot.health < 20, 3000)
    await h.ticks(3)
    const b = await food()
    h.result(`ex.damage.${type}`, `${exDelta(a, b)} health=${r2(bot.health)}`)
  }
  await resetPlayer()
  h.cmd(`effect give ${bot.username} absorption 30 0`)
  await h.ticks(5)
  {
    const a = await food()
    h.cmd(`damage ${bot.username} 2 minecraft:mob_attack`)
    await h.ticks(5)
    const b = await food()
    h.result('ex.damage.absorbed', `${exDelta(a, b)} health=${r2(bot.health)} absorption=${await num('AbsorptionAmount')}`)
  }

  // Attacks: a landed hit, a swing at nothing.
  await resetPlayer()
  await stand()
  h.cmd(`summon husk ${X + 0.5} ${Y + 1} ${Z + 2.5} {NoAI:1b,PersistenceRequired:1b,Silent:1b}`)
  await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'husk'), 3000)
  await h.ticks(20)
  {
    const husk = Object.values(bot.entities).find(e => e.name === 'husk')
    await bot.lookAt(husk.position.offset(0, 1, 0), true)
    const a = await food()
    bot.attack(husk)
    await h.ticks(5)
    const b = await food()
    h.result('ex.attack', exDelta(a, b))
    h.cmd('kill @e[type=husk]')
    await h.ticks(25)
    const c = await food()
    bot.swingArm()
    await h.ticks(5)
    const d = await food()
    h.result('ex.swing_air', exDelta(c, d))
  }

  // Mining: only a block that drops with the tool used exhausts and counts as mined.
  for (const [block, tool, key] of [['dirt', null, 'dirt_hand'], ['netherrack', null, 'netherrack_hand'], ['netherrack', 'wooden_pickaxe', 'netherrack_pickaxe']]) {
    await resetPlayer()
    await h.clear()
    if (tool) {
      await h.give(tool)
      await bot.equip(bot.inventory.items().find(i => i.name === tool), 'hand')
    }
    await stand()
    h.cmd(`setblock ${X} ${Y + 1} ${Z + 2} ${block}`)
    await h.waitFor(() => h.blockName(X, Y + 1, Z + 2) === block, 3000)
    await h.ticks(5)
    const a = await food(); const s0 = await readStats()
    await bot.dig(h.block(X, Y + 1, Z + 2), true).catch(e => h.result(`mine.${key}.error`, e.message))
    await h.ticks(5)
    const b = await food(); const s1 = await readStats()
    const minedKeys = Object.keys(s1).filter(k => k.startsWith('mined:') && statDelta(s0, s1, k) !== 0)
    h.result(`ex.mine.${key}`, `${exDelta(a, b)} mined_stats_changed=${minedKeys.length}`)
    h.cmd('kill @e[type=item]')
  }
  await h.clear()

  // Effects that work the food bar.
  await resetPlayer()
  {
    const a = await food()
    h.cmd(`effect give ${bot.username} hunger 5 1`)
    await h.ticks(130)
    const b = await food()
    h.result('ex.hunger_effect_5s_amp1', Math.round(exDelta(a, b) * 100) / 100)
  }
  h.cmd(`effect give ${bot.username} hunger 30 100`)
  await h.waitFor(() => bot.food <= 10, 30000)
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(10)
  {
    const a = await food()
    // Instant effects take their duration in ticks: three ticks of Saturation I.
    h.cmd(`effect give ${bot.username} saturation 3 0`)
    await h.ticks(10)
    const b = await food()
    h.result('saturation_effect_3ticks', `food+${b.food - a.food} sat+${r2(b.sat - a.sat)}`)
  }
}

async function starvationScenes () {
  for (const [difficulty, hp] of [['easy', 11.5], ['normal', 3.5], ['hard', 2]]) {
    h.cmd(`difficulty ${difficulty}`)
    await setHealth(20)
    h.cmd(`effect give ${bot.username} hunger 60 120`)
    await h.waitFor(() => bot.food === 0, 30000)
    h.cmd(`effect clear ${bot.username}`)
    await h.ticks(5)
    h.cmd(`damage ${bot.username} ${20 - hp} minecraft:generic_kill`)
    await h.waitFor(() => bot.health < 20, 3000)
    const result = await hurtsDuring(330)
    h.result(`starve.${difficulty}`, `${result} food=${bot.food}`)
    if (bot.health <= 0) await h.respawnAfterDeath()
  }
  // Peaceful fills food and saturation and heals.
  h.cmd('difficulty normal')
  h.cmd('gamerule natural_health_regeneration true')
  await setHealth(20)
  h.cmd(`effect give ${bot.username} hunger 30 120`)
  await h.waitFor(() => bot.food <= 4, 30000)
  h.cmd(`effect clear ${bot.username}`)
  h.cmd(`damage ${bot.username} 10 minecraft:generic_kill`)
  await h.ticks(5)
  h.cmd('difficulty peaceful')
  await h.ticks(100)
  const f = await food()
  // 100 ticks: 5 hearts' worth of half points and 10 food, give or take one tick's phase.
  h.result('peaceful.100ticks', `health>=14=${bot.health >= 14} food>=13=${f.food >= 13} sat>0=${f.sat > 0}`)
  h.cmd('difficulty normal')
  h.cmd('gamerule natural_health_regeneration false')
}

async function hazardScenes () {
  // A death in the starvation scenes respawned the bot far away: come back and let the
  // arena's chunks load before building in them.
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await clearArena()
  // A glass tank of water three deep for drowning.
  const TX = X + 6; const TZ = Z + 6
  h.cmd(`fill ${TX - 2} ${Y + 1} ${TZ - 2} ${TX + 2} ${Y + 5} ${TZ + 2} glass hollow`)
  h.cmd(`fill ${TX - 1} ${Y + 1} ${TZ - 1} ${TX + 1} ${Y + 4} ${TZ + 1} water`)
  h.cmd(`setblock ${TX} ${Y + 5} ${TZ} glass`)
  await h.waitFor(() => h.blockName(TX, Y + 4, TZ) === 'water', 5000)
  const intoTank = async () => {
    h.cmd(`tp ${bot.username} ${TX + 0.5} ${Y + 1} ${TZ + 0.5}`)
    await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(TX + 0.5, Y + 1, TZ + 0.5)) < 1, 5000)
    await h.ticks(5)
  }
  await setHealth(20)
  await intoTank()
  h.result('drown.in_tank', `${h.blockName(TX, Y + 2, TZ)} y=${Math.round(bot.entity.position.y * 10) / 10}`)
  await h.ticks(60)
  const air60 = await num('Air')
  h.result('drown.air_after_60', air60 > 200 && air60 < 260 ? 'between' : air60)
  // Wait for the first drowning hit, then count the hits in the next 100 ticks.
  await h.waitFor(() => bot.health < 20, 30000)
  const airAfterHit = await num('Air')
  h.result('drown.air_after_first_hit', airAfterHit <= 0 && airAfterHit > -15 ? 'reset' : airAfterHit)
  h.result('drown.first_hit', r2(bot.health))
  h.result('drown.next_100', await hurtsDuring(100))
  // Water breathing keeps the air and refills it.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} water_breathing 60 0`)
  await h.ticks(100)
  h.result('drown.water_breathing_air', await num('Air'))
  h.cmd(`effect clear ${bot.username}`)
  // Out of the water the air comes back four a tick.
  await h.ticks(100)
  await stand()
  await h.ticks(80)
  h.result('drown.air_refilled', await num('Air'))
  // A turtle shell gives ten seconds of water breathing, renewed while out of the water.
  h.cmd(`item replace entity ${bot.username} armor.head with turtle_helmet`)
  await h.ticks(10)
  h.result('turtle.effect_on_land', (await nbt('active_effects[{id:"minecraft:water_breathing"}].duration')) || 'none')
  await intoTank()
  await h.ticks(60)
  h.result('turtle.air_in_water_60', await num('Air'))
  await stand()
  await h.clear()
  h.cmd(`item replace entity ${bot.username} armor.head with air`)
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(5)
  h.cmd(`fill ${TX - 2} ${Y + 1} ${TZ - 2} ${TX + 2} ${Y + 5} ${TZ + 2} air`)

  // Suffocation in a full block, none in glass.
  for (const block of ['stone', 'glass', 'sand']) {
    await setHealth(20)
    await stand()
    h.cmd(`setblock ${X} ${Y + 2} ${Z} ${block}`)
    h.result(`suffocate.${block}`, await hurtsDuring(60))
    h.cmd(`setblock ${X} ${Y + 2} ${Z} air`)
    await h.ticks(5)
  }

  // Fire: a player only catches fire after about a second in it, then burns on after.
  for (const ticks of [10, 40]) {
    await setHealth(20)
    await stand()
    h.cmd(`setblock ${X} ${Y + 1} ${Z} fire`)
    await h.ticks(ticks)
    h.cmd(`setblock ${X} ${Y + 1} ${Z} air`)
    await h.ticks(2)
    const fire = await num('Fire')
    h.result(`fire.${ticks}.burning_after`, fire > 0)
    // Health after the long burn depends on how many in-fire hits landed, which varies by a tick.
    h.result(`fire.${ticks}.burning_80`, (await hurtsDuring(80)).replace(/ health=.*/, ''))
  }
  // Water puts the fire out.
  h.cmd(`setblock ${X} ${Y + 1} ${Z} water`)
  await h.ticks(10)
  h.result('fire.after_water', await num('Fire'))
  h.cmd(`setblock ${X} ${Y + 1} ${Z} air`)
  await h.ticks(20)

  // Rain puts a burning mob out, with the extinguish sound (raw sound ids, as in ambient.js).
  h.cmd(`fillbiome ${X - 6} ${Y - 4} ${Z - 6} ${X + 6} ${Y + 8} ${Z + 6} minecraft:plains`)
  h.cmd(`summon pig ${X + 3.5} ${Y + 1} ${Z + 3.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b,Fire:600s}`)
  await h.waitFor(() => Object.values(bot.entities).some((e) => e.name === 'pig'), 3000)
  await h.ticks(10)
  const rainPig = Object.values(bot.entities).find((e) => e.name === 'pig')
  const rainSounds = new Set()
  const onRainSound = (p) => { const s = p.sound; rainSounds.add(s && s.data ? s.data.soundName : String(s ? s.soundId : p.soundId)) }
  bot._client.on('sound_effect', onRainSound)
  const burningBefore = rainPig && rainPig.metadata ? (rainPig.metadata[0] & 1) !== 0 : 'missing'
  // The rain level needs about 20 ticks to count as raining.
  h.cmd('weather rain')
  await h.serverTicks(60)
  bot._client.removeListener('sound_effect', onRainSound)
  const burningAfter = rainPig && rainPig.metadata ? (rainPig.metadata[0] & 1) !== 0 : 'missing'
  h.result('rain.pig', `before=${burningBefore} after=${burningAfter} sounds=${[...rainSounds].sort().join(',') || 'none'}`)
  h.cmd('weather clear')
  h.cmd('kill @e[type=pig]')
  await h.ticks(20)

  // Lava, with and without fire resistance.
  for (const resist of [true, false]) {
    await setHealth(20)
    await stand()
    if (resist) h.cmd(`effect give ${bot.username} fire_resistance 60 0`)
    await h.ticks(5)
    h.cmd(`setblock ${X} ${Y + 1} ${Z} lava[level=8]`)
    const lava = await hurtsDuring(30)
    h.cmd(`fill ${X - 3} ${Y + 1} ${Z - 3} ${X + 3} ${Y + 1} ${Z + 3} air`)
    await h.ticks(3)
    const f = await num('Fire')
    h.result(`lava.${resist ? 'fire_resistance' : 'plain'}`, `${lava} fire=${f >= 280 ? '>=280' : f}`)
    await h.waitFor(() => bot.health <= 0 || true, 10)
    if (bot.health <= 0) {
      await h.respawnAfterDeath()
      await h.tp(X + 0.5, Y + 1, Z + 0.5)
    }
    h.cmd(`effect clear ${bot.username}`)
    h.cmd(`setblock ${X} ${Y + 1} ${Z} water`)
    await h.ticks(10)
    h.cmd(`setblock ${X} ${Y + 1} ${Z} air`)
    await h.ticks(10)
  }

  // Powder snow: freezing, damage once fully frozen, leather armor.
  for (const armor of [null, 'leather_chestplate']) {
    await setHealth(20)
    await h.clear()
    if (armor) h.cmd(`item replace entity ${bot.username} armor.chest with ${armor}`)
    await stand()
    h.cmd(`fill ${X} ${Y + 1} ${Z} ${X} ${Y + 2} ${Z} powder_snow`)
    await h.ticks(60)
    const t60 = (await num('TicksFrozen')) || 0
    const key = armor ? 'powder_snow.leather' : 'powder_snow.plain'
    h.result(`${key}.frozen_60`, t60 >= 50 && t60 <= 70 ? '50-70' : t60)
    if (!armor) {
      await h.waitFor(() => bot.health < 20, 10000)
      h.result(`${key}.first_hit_frozen`, await num('TicksFrozen'))
      h.result(`${key}.next_100`, await hurtsDuring(100))
    }
    h.cmd(`fill ${X} ${Y + 1} ${Z} ${X} ${Y + 2} ${Z} air`)
    await h.ticks(20)
    const thaw = (await num('TicksFrozen')) || 0
    h.result(`${key}.thawing`, armor ? thaw : (thaw >= 90 && thaw <= 110 ? '90-110' : thaw))
    await h.ticks(80)
  }
  await h.clear()

  // Magma blocks, sneaking on them, and fire resistance.
  for (const mode of ['plain', 'sneak', 'fire_resistance']) {
    await setHealth(20)
    await stand()
    if (mode === 'fire_resistance') h.cmd(`effect give ${bot.username} fire_resistance 60 0`)
    if (mode === 'sneak') bot.setControlState('sneak', true)
    await h.ticks(5)
    h.cmd(`setblock ${X} ${Y} ${Z} magma_block`)
    h.result(`magma.${mode}`, await hurtsDuring(60))
    bot.clearControlStates()
    h.cmd(`setblock ${X} ${Y} ${Z} stone`)
    h.cmd(`effect clear ${bot.username}`)
    await h.ticks(10)
  }

  // A sweet berry bush doesn't hurt a player standing still in it.
  await setHealth(20)
  await stand()
  h.cmd(`setblock ${X} ${Y + 1} ${Z} sweet_berry_bush[age=3]`)
  h.result('berry.still', await hurtsDuring(40))
  h.cmd(`setblock ${X} ${Y + 1} ${Z} air`)
  await h.ticks(10)

  // Cactus: standing against one, inside its block but outside its collision box.
  await setHealth(20)
  await stand()
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 1} cactus`)
  await h.ticks(5)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.76}`)
  const cactus = await hurtsDuring(40)
  h.result('cactus.touching', cactus)
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 1} air`)
  await h.ticks(10)

  // The void: four damage a tick far below the world's bottom.
  await setHealth(20)
  const v0 = healths.length
  h.cmd(`tp ${bot.username} ${X + 0.5} -140 ${Z + 0.5}`)
  await h.waitFor(() => bot.health <= 0, 10000)
  const voidSeen = healths.slice(v0)
  h.result('void.updates_to_death', `${voidSeen.length} first=${voidSeen[0]}`)
  if (bot.health <= 0) await h.respawnAfterDeath()
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await stand()
}

async function effectScenes () {
  const effectList = async () => {
    const v = await nbt('active_effects')
    if (v === null) return 'none'
    const ids = [...v.matchAll(/id: "minecraft:([a-z_]+)"/g)].map(m => m[1])
    const amps = [...v.matchAll(/amplifier: (-?\d+)b/g)].map(m => m[1])
    const durs = [...v.matchAll(/duration: (-?\d+)/g)].map(m => Math.round(Number(m[1]) / 100) * 100)
    return ids.map((id, i) => `${id}`).join(',') + ` amps=${amps.join(',') || '0'} dur~${durs.join(',')}`
  }

  // Poison: one point every 25 ticks at level I, never below one health.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} poison 5 0`)
  h.result('poison.5s_full', await hurtsDuring(110))
  await setHealth(2.5)
  h.cmd(`effect give ${bot.username} poison 5 0`)
  await h.ticks(110)
  h.result('poison.from_2.5', r2(bot.health))
  // Wither: every 40 ticks at level I, all the way down.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} wither 5 0`)
  h.result('wither.5s', await hurtsDuring(110))
  // Regeneration: every 50 ticks at level I, 25 at level II.
  for (const amp of [0, 1]) {
    await setHealth(10)
    h.cmd(`effect give ${bot.username} regeneration 5 ${amp}`)
    await h.ticks(110)
    h.result(`regeneration.5s_amp${amp}`, r2(bot.health))
  }

  // Absorption: hearts on start, the effect ends once they're gone.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} absorption 30 0`)
  await h.ticks(5)
  h.result('absorption.amount', await num('AbsorptionAmount'))
  h.cmd(`damage ${bot.username} 5 minecraft:generic`)
  await h.ticks(10)
  h.result('absorption.after_5_damage', `health=${r2(bot.health)} absorption=${await num('AbsorptionAmount')}`)
  h.result('absorption.effects_after_depleted', await effectList())
  // A weaker absorption after a stronger one keeps the bigger amount.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} absorption 10 1`)
  await h.ticks(3)
  h.cmd(`effect give ${bot.username} absorption 60 0`)
  await h.ticks(3)
  h.result('absorption.weaker_after_stronger', `${await num('AbsorptionAmount')} ${await effectList()}`)
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(3)
  h.result('absorption.cleared', await num('AbsorptionAmount'))

  // Reapplying: a stronger shorter effect hides the weaker longer one, which comes back.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} speed 30 0`)
  await h.ticks(3)
  h.cmd(`effect give ${bot.username} speed 5 2`)
  await h.ticks(3)
  h.result('reapply.stronger_shorter', await effectList())
  await h.ticks(120)
  h.result('reapply.hidden_resumes', await effectList())
  h.cmd(`effect give ${bot.username} speed 10 0`)
  await h.ticks(3)
  h.result('reapply.same_amp_shorter', await effectList())
  h.cmd(`effect give ${bot.username} speed 60 0`)
  await h.ticks(3)
  h.result('reapply.same_amp_longer', await effectList())
  h.cmd(`effect give ${bot.username} speed infinite 0`)
  await h.ticks(3)
  h.result('reapply.infinite', await nbt('active_effects[0].duration'))
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(3)

  // Slow falling cancels fall damage.
  await setHealth(20)
  h.cmd(`effect give ${bot.username} slow_falling 30 0`)
  await h.ticks(3)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 12} ${Z + 0.5}`)
  await h.waitFor(() => bot.entity.position.y > Y + 10, 3000)
  await h.waitFor(() => bot.entity.onGround && bot.entity.position.y < Y + 2, 20000)
  await h.ticks(10)
  h.result('slow_falling.fall_11', r2(bot.health))
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(5)
}

async function sleepScenes () {
  await clearArena()
  await setHealth(20)
  await h.clear()
  h.cmd('time set 18000')
  h.cmd(`setblock ${X + 2} ${Y + 1} ${Z} red_bed[facing=east,part=foot]`)
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z} red_bed[facing=east,part=head]`)
  await h.waitFor(() => h.blockName(X + 3, Y + 1, Z) === 'red_bed', 5000)
  // Too far away: more than a few blocks off.
  await stand(X - 2, Z)
  takeTranslated()
  await bot.activateBlock(h.block(X + 2, Y + 1, Z)).catch(() => {})
  await h.ticks(10)
  h.result('bed.too_far', takeTranslated())
  // Obstructed: a block above the head.
  await stand()
  h.cmd(`setblock ${X + 3} ${Y + 2} ${Z} stone`)
  await h.ticks(5)
  takeTranslated()
  await bot.activateBlock(h.block(X + 2, Y + 1, Z)).catch(() => {})
  await h.ticks(10)
  h.result('bed.obstructed', `${takeTranslated()} sleeping=${bot.isSleeping}`)
  h.cmd(`setblock ${X + 3} ${Y + 2} ${Z} air`)
  await h.ticks(5)
  // Sleeping resets time_since_rest and counts sleep_in_bed.
  const s0 = await readStats()
  await bot.sleep(h.block(X + 2, Y + 1, Z)).catch(e => h.result('bed.sleep_error', e.message))
  await h.ticks(20)
  const s1 = await readStats()
  h.result('bed.stats_while_asleep', `rest=${s1.time_since_rest} rest_before>0=${(s0.time_since_rest || 0) > 0} sleep_in_bed+${statDelta(s0, s1, 'sleep_in_bed')}`)
  await h.waitFor(() => !bot.isSleeping, 15000)
  await h.ticks(20)
  const s2 = await readStats()
  h.result('bed.rest_after_wake', s2.time_since_rest > 0 && s2.time_since_rest < 200 ? 'small' : s2.time_since_rest)
  h.result('bed.morning', bot.time.timeOfDay < 1000)

  // players_sleeping_percentage 0 skips the night as soon as anyone sleeps for long enough;
  // above 100 no night is ever skipped.
  h.cmd('gamerule players_sleeping_percentage 101')
  h.cmd('time set 18000')
  await h.ticks(10)
  takeTranslated()
  await bot.sleep(h.block(X + 2, Y + 1, Z)).catch(e => h.result('bed.pct101_error', e.message))
  await h.ticks(140)
  h.result('bed.pct101', `still_night=${bot.time.timeOfDay > 12000} sleeping=${bot.isSleeping} msgs=${takeTranslated()}`)
  // Daybreak without the night skipped throws the sleeper out of bed.
  h.cmd('time set 1000')
  await h.ticks(20)
  h.result('bed.daybreak_wakes', `sleeping=${bot.isSleeping}`)
  await bot.wake().catch(() => {})
  await h.waitFor(() => !bot.isSleeping, 5000)
  h.cmd('gamerule players_sleeping_percentage 100')
  h.cmd('time set 1000')
  await h.ticks(10)
}

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd('difficulty normal')
  h.cmd('gamerule fall_damage true')
  await h.ticks(10)
  await exhaustionScenes()
  await starvationScenes()
  await hazardScenes()
  await effectScenes()
  await sleepScenes()
  h.cmd('gamerule natural_health_regeneration true')
})
