// Mob and item behaviour that shows in entity data and positions: health read from NBT,
// axolotls drying out on land, bounces off slime blocks, dispensed shears cutting leashes and
// emptying beehives, spawn egg babies taking the parent's variant, drowning, wolf variants and
// taming health, players emptying beehives, and the Peaceful despawn.
const { make } = require('./survival-lib')
const h = make('Gaps')
const { bot } = h
const X = 9900; const Y = 200; const Z = 0

const meta = new Map()
// Killed mobs linger for their death animation; leave them out.
const named = (name) => Object.values(bot.entities).filter(e => e !== bot.entity && e.name === name && (meta.get(e.id) || new Map()).get(9) !== 0)
const show = (v) => (typeof v === 'object' ? JSON.stringify(v) : String(v))
const values = (e, skip = []) => {
  const m = meta.get(e.id) || new Map()
  return [...m.keys()].filter(k => !skip.includes(k)).sort((a, b) => a - b).map(k => `${k}=${show(m.get(k))}`).join(' ') || 'none'
}
const place = async (x, y, z, block) => {
  const name = block.split('[')[0].split('{')[0]
  for (let i = 0; i < 5; i++) {
    h.cmd(`setblock ${x} ${y} ${z} ${block}`)
    if (await h.waitFor(() => h.blockName(x, y, z) === name, 2000)) break
  }
}
// Only the named drops: the terrain around the arena drops its own items.
const drops = (names) => named('item').map(e => e.getDroppedItem()).filter(it => it && names.includes(it.name)).map(h.stackStr).sort().join(',') || 'none'
// Bees from nests in the terrain below can fly into view; only those near the hive count.
const nearHive = (x, z) => named('bee').filter(b => b.position.distanceTo(new h.Vec3(x + 0.5, Y + 1.5, z + 0.5)) < 8)
const clean = async () => {
  h.cmd(`kill @e[type=!player,distance=..30,x=${X},y=${Y},z=${Z}]`)
  await h.ticks(5)
}

h.start(async () => {
  bot._client.on('entity_metadata', (p) => {
    if (!meta.has(p.entityId)) meta.set(p.entityId, new Map())
    for (const m of p.metadata) meta.get(p.entityId).set(m.key, m.value)
  })
  await h.arena(X, Y, Z, 8)
  h.cmd('time set noon')
  h.cmd('gamerule advance_time false')
  h.cmd('weather clear')
  h.cmd('difficulty normal')

  // Health a summon reads from its NBT reaches the client.
  for (const [type, nbt] of [['slime', 'Size:3'], ['hoglin', ''], ['pig', 'Health:5f']]) {
    await clean()
    h.cmd(`summon ${type} ${X + 0.5} ${Y + 1} ${Z + 4.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b${nbt ? ',' + nbt : ''}}`)
    await h.waitFor(() => named(type).length > 0, 3000)
    await h.ticks(5)
    const e = named(type)[0]
    h.result(`health.${type}`, e ? show((meta.get(e.id) || new Map()).get(9)) : 'missing')
  }

  // An axolotl on land loses one air a tick and takes 2 damage each time it dries out.
  await clean()
  h.cmd(`summon axolotl ${X + 0.5} ${Y + 1} ${Z - 4.5} {Air:10s,Silent:1b,PersistenceRequired:1b}`)
  await h.waitFor(() => named('axolotl').length > 0, 3000)
  const axolotl = named('axolotl')[0]
  // The first dry out lands 30 ticks in, too close to a 20 tick clock to time it; wait for it.
  const health = () => axolotl ? (meta.get(axolotl.id) || new Map()).get(9) : undefined
  await h.waitFor(() => health() !== undefined && health() < 14, 5000)
  h.result('axolotl.health', axolotl ? show(health()) : 'missing')

  // Highest point after landing on a block from five blocks up.
  // Item positions aren't sent while they fly, so only living entities are checked.
  const bounce = async (key, type, block, nbt = '') => {
    await clean()
    await place(X + 4, Y, Z, block)
    h.cmd(`summon ${type} ${X + 4.5} ${Y + 6} ${Z + 0.5} {${nbt}}`)
    await h.waitFor(() => named(type).length > 0, 3000)
    const e = named(type)[0]
    if (!e) { h.result(`bounce.${key}`, 'missing'); return }
    let landed = false; let peak = -Infinity
    const end = Date.now() + 4000
    while (Date.now() < end) {
      const y = e.position.y - Y
      if (!landed && y < 1.1) landed = true
      else if (landed) peak = Math.max(peak, y)
      await h.ticks(1)
    }
    h.result(`bounce.${key}`, landed ? peak.toFixed(2) : 'never landed')
    await place(X + 4, Y, Z, 'stone')
  }
  await bounce('armor_stand_slime', 'armor_stand', 'slime_block')
  await bounce('armor_stand_honey', 'armor_stand', 'honey_block')
  await bounce('armor_stand_stone', 'armor_stand', 'stone')

  // Dispensed shears cut a pig's leash to a fence; the knot goes with its last leash.
  await clean()
  const LZ = Z - 3
  await place(X - 5, Y + 1, LZ, 'oak_fence')
  h.cmd(`summon pig ${X - 1.5} ${Y + 1} ${LZ + 0.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b,leash:[I;${X - 5},${Y + 1},${LZ}]}`)
  await h.waitFor(() => named('leash_knot').length > 0, 3000)
  h.result('leash.knots_before', named('leash_knot').length)
  await place(X - 1, Y + 1, LZ, 'dispenser[facing=west]')
  h.cmd(`item replace block ${X - 1} ${Y + 1} ${LZ} container.0 with shears`)
  await h.ticks(2)
  await place(X, Y + 1, LZ, 'redstone_block')
  await h.serverTicks(10)
  h.result('leash.knots_after', named('leash_knot').length)
  h.result('leash.items', drops(['lead']))
  await place(X, Y + 1, LZ, 'air')
  await place(X - 1, Y + 1, LZ, 'air')
  await place(X - 5, Y + 1, LZ, 'air')

  // Dispensed shears empty a full beehive and let its bee out at noon.
  await clean()
  await place(X - 3, Y + 1, Z + 5, 'beehive[facing=west,honey_level=5]{bees:[{entity_data:{id:"minecraft:bee"},ticks_in_hive:0,min_ticks_in_hive:600}]}')
  await place(X - 2, Y + 1, Z + 5, 'dispenser[facing=west]')
  h.cmd(`item replace block ${X - 2} ${Y + 1} ${Z + 5} container.0 with shears`)
  await h.ticks(2)
  await place(X - 1, Y + 1, Z + 5, 'redstone_block')
  await h.serverTicks(10)
  const hive = h.block(X - 3, Y + 1, Z + 5)
  h.result('hive.honey_level', hive ? hive.getProperties().honey_level : 'unloaded')
  h.result('hive.bees', nearHive(X - 3, Z + 5).length)
  h.result('hive.items', drops(['honeycomb', 'lead']))
  await place(X - 1, Y + 1, Z + 5, 'air')
  await place(X - 2, Y + 1, Z + 5, 'air')
  await place(X - 3, Y + 1, Z + 5, 'air')

  // A spawn egg used on a mob of its kind makes a baby with the parent's variant.
  for (const [type, nbt] of [['cow', 'variant:"minecraft:cold"'], ['cat', 'variant:"minecraft:calico"']]) {
    await clean()
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
    h.cmd(`summon ${type} ${X + 0.5} ${Y + 1} ${Z + 2.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b,${nbt}}`)
    await h.waitFor(() => named(type).length > 0, 3000)
    const parent = named(type)[0]
    await h.clear()
    await h.give(`${type}_spawn_egg`)
    const egg = bot.inventory.items().find(it => it.name === `${type}_spawn_egg`)
    if (egg) await bot.equip(egg, 'hand')
    await bot.lookAt(parent.position.offset(0, 0.5, 0), true)
    await bot.activateEntity(parent).catch(() => {})
    await h.waitFor(() => named(type).length > 1, 3000)
    await h.ticks(5)
    const baby = named(type).find(e => e !== parent)
    h.result(`egg.${type}`, baby ? values(baby, [9]) : 'none')
  }

  // Underwater a pig loses one air a tick and takes 2 drowning damage at -20 air; a zombie
  // breathes and refills four a tick.
  await clean()
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  h.cmd(`fill ${X + 4} ${Y + 1} ${Z - 7} ${X + 8} ${Y + 5} ${Z - 5} glass`)
  h.cmd(`fill ${X + 5} ${Y + 1} ${Z - 6} ${X + 5} ${Y + 4} ${Z - 6} water`)
  h.cmd(`fill ${X + 7} ${Y + 1} ${Z - 6} ${X + 7} ${Y + 4} ${Z - 6} water`)
  await h.waitFor(() => h.blockName(X + 7, Y + 4, Z - 6) === 'water', 3000)
  h.cmd(`summon pig ${X + 5.5} ${Y + 1} ${Z - 5.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b,Air:10s}`)
  h.cmd(`summon zombie ${X + 7.5} ${Y + 1} ${Z - 5.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b,Air:10s}`)
  await h.waitFor(() => named('pig').length > 0 && named('zombie').length > 0, 3000)
  await h.serverTicks(40)
  for (const type of ['pig', 'zombie']) {
    const e = named(type)[0]
    const m = (e && meta.get(e.id)) || new Map()
    // Exact air depends on when the 20 tick clock fired; only its direction is checked.
    h.result(`drown.${type}`, e ? `health=${show(m.get(9))} air_${type === 'pig' ? 'below_zero' : 'refilled'}=${type === 'pig' ? m.get(1) < 0 : m.get(1) > 10}` : 'missing')
  }
  await clean()
  h.cmd(`fill ${X + 4} ${Y + 1} ${Z - 7} ${X + 8} ${Y + 5} ${Z - 5} air`)

  // A player under water runs out of air and drowns two health at a time.
  h.cmd(`fill ${X - 8} ${Y + 1} ${Z + 6} ${X - 6} ${Y + 5} ${Z + 8} glass`)
  h.cmd(`fill ${X - 7} ${Y + 1} ${Z + 7} ${X - 7} ${Y + 4} ${Z + 7} water`)
  await h.waitFor(() => h.blockName(X - 7, Y + 4, Z + 7) === 'water', 3000)
  h.cmd(`tp ${bot.username} ${X - 6.5} ${Y + 1} ${Z + 7.5}`)
  const drowned = await h.waitFor(() => bot.health < 20, 25000)
  // The health packet can come before the same tick's air update.
  await h.ticks(2)
  h.result('drown.player', drowned ? `health=${bot.health} oxygen=${bot.oxygenLevel}` : `health=${bot.health}`)
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  h.cmd(`fill ${X - 8} ${Y + 1} ${Z + 6} ${X - 6} ${Y + 5} ${Z + 8} air`)
  h.cmd(`effect give ${bot.username} instant_health 1 5`)
  await h.ticks(10)

  // A summoned wolf takes the variant of its biome; a wolf tamed with bones gets 40 health.
  for (const [biome, dx] of [['snowy_taiga', -5], ['forest', 5]]) {
    await clean()
    h.cmd(`fillbiome ${X + dx - 3} ${Y - 4} ${Z - 3} ${X + dx + 3} ${Y + 8} ${Z + 3} minecraft:${biome}`)
    await h.ticks(5)
    h.cmd(`summon wolf ${X + dx + 0.5} ${Y + 1} ${Z + 0.5}`)
    await h.waitFor(() => named('wolf').length > 0, 3000)
    await h.ticks(5)
    const wolf = named('wolf')[0]
    h.result(`wolf.variant_${biome}`, wolf ? show((meta.get(wolf.id) || new Map()).get(23)) : 'missing')
  }
  await clean()
  h.cmd(`summon wolf ${X + 0.5} ${Y + 1} ${Z + 2.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b}`)
  await h.waitFor(() => named('wolf').length > 0, 3000)
  const wolf = named('wolf')[0]
  await h.clear()
  await h.give('bone', 64)
  await bot.equip(bot.inventory.items().find(it => it.name === 'bone'), 'hand')
  const tamed = () => ((meta.get(wolf.id) || new Map()).get(18) & 4) !== 0
  for (let i = 0; i < 40 && wolf && !tamed(); i++) {
    await bot.lookAt(wolf.position.offset(0, 0.4, 0), true)
    await bot.activateEntity(wolf).catch(() => {})
    await h.ticks(3)
  }
  h.result('wolf.tamed_health', wolf && tamed() ? show((meta.get(wolf.id) || new Map()).get(9)) : 'not tamed')

  // A player emptying a full hive lets its bee out angry at them, unless a campfire smokes it.
  for (const [key, item, smoked] of [['bottle', 'glass_bottle', false], ['shears', 'shears', false], ['smoked', 'glass_bottle', true]]) {
    await clean()
    await h.clear()
    await h.tp(X + 0.5, Y + 1, Z + 0.5)
    if (smoked) await place(X - 3, Y + 1, Z + 3, 'campfire')
    await place(X - 3, Y + (smoked ? 2 : 1), Z + 3, 'beehive[facing=east,honey_level=5]{bees:[{entity_data:{id:"minecraft:bee"},ticks_in_hive:0,min_ticks_in_hive:600}]}')
    const hy = Y + (smoked ? 2 : 1)
    await h.give(item)
    await bot.equip(bot.inventory.items().find(it => it.name === item), 'hand')
    await bot.lookAt(new h.Vec3(X - 2.5, hy + 0.5, Z + 3.5), true)
    await bot.activateBlock(h.block(X - 3, hy, Z + 3), new h.Vec3(1, 0, 0)).catch(e => h.result(`hive_${key}.error`, e.message))
    await h.serverTicks(20)
    const hiveState = h.block(X - 3, hy, Z + 3)
    const bees = nearHive(X - 3, Z + 3)
    // The anger end time is only sent once it leaves its default.
    const angry = bees.filter(b => (meta.get(b.id) || new Map()).has(19)).length
    h.result(`hive_${key}`, `honey=${hiveState ? hiveState.getProperties().honey_level : 'unloaded'} bees=${bees.length} angry=${angry} inv=${h.invStr()} drops=${drops(['honeycomb'])}`)
    await place(X - 3, Y + 1, Z + 3, 'air')
    await place(X - 3, Y + 2, Z + 3, 'air')
  }

  // Peaceful removes what may not live there, persistent or not.
  await clean()
  for (const [i, type] of ['zombie', 'piglin', 'slime', 'shulker', 'ghast'].entries()) {
    h.cmd(`summon ${type} ${X - 6 + 3 * i + 0.5} ${Y + 1} ${Z - 6.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b}`)
  }
  await h.ticks(10)
  h.cmd('difficulty peaceful')
  await h.serverTicks(10)
  h.result('peaceful.left', ['zombie', 'piglin', 'slime', 'shulker', 'ghast'].filter(t => named(t).length > 0).join(',') || 'none')
  h.cmd('difficulty normal')
  await clean()
})
