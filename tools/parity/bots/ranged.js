// Ranged weapons fired by a survival player: bow damage by charge and crit arrows, arrow pickup
// rules, snowball and egg knockback, trident damage and loyalty, crossbow charge time and multishot.
const { make } = require('./survival-lib')
const h = make('Ranger')
const { bot } = h
const X = 2000; const Y = 200; const Z = 0
const HEALTH = 9
const r3 = (n) => Math.round(n * 1000) / 1000

let tick = 0
bot.on('physicsTick', () => { tick++ })
const velocities = {}
bot._client.on('entity_velocity', (p) => {
  const v = p.velocity
  velocities[p.entityId] = `${v.x.toFixed(2)},${v.y.toFixed(2)},${v.z.toFixed(2)}`
})

const health = (e, max) => (e.metadata && typeof e.metadata[HEALTH] === 'number') ? e.metadata[HEALTH] : max
const named = (name) => Object.values(bot.entities).filter(e => e.name === name)

async function clearMobs () {
  h.cmd('tp @e[type=!player] ~ -300 ~')
  h.cmd('kill @e[type=!player]')
  await h.ticks(5)
}

async function spawn (type, dz, extra = '') {
  h.cmd(`summon ${type} ${X + 0.5} ${Y + 1} ${(Z + 0.5 + dz).toFixed(3)} {NoAI:1b,Silent:1b,PersistenceRequired:1b${extra}}`)
  let e
  await h.waitFor(() => (e = named(type).find(m => Math.abs(m.position.z - (Z + 0.5 + dz)) < 0.6)), 5000)
  await h.ticks(2)
  return e
}

async function give (items) {
  h.cmd(`clear ${bot.username}`)
  await h.ticks(2)
  items.forEach((it, i) => h.cmd(`item replace entity ${bot.username} hotbar.${i} with ${it}`))
  bot.setQuickBarSlot(0)
  await h.waitFor(() => bot.inventory.items().length === items.length, 5000)
  await h.ticks(10)
}

async function stand () {
  await h.tp(X + 0.5, Y + 1, Z + 0.5, 0, 0)
  await h.ticks(5)
}

// Charges the held item for `ticks` ticks, aiming at the middle of `target`, and lets go.
async function shoot (target, ticks, aimY = 1.3) {
  await bot.lookAt(target.position.offset(0, aimY, 0), true)
  bot.activateItem()
  await h.ticks(ticks)
  bot.deactivateItem()
}

h.start(async () => {
  await h.arena(X, Y, Z, 10)
  h.cmd('difficulty peaceful')
  h.cmd('gamerule mob_drops false')
  await stand()

  // A full draw at 3 blocks: always a crit, whose bonus is random, so only the range is compared.
  // The shot's spread can push the speed just over 3, so Power V reaches ceil(3.0x * 5) = 16
  // plus a crit bonus of up to 9.
  // Shorter draws depend on the exact tick the release lands on, which drifts between servers.
  for (const [label, bow] of [['bow_full', 'bow'], ['bow_power5_full', 'bow[enchantments={power:5}]']]) {
    await clearMobs()
    await give([bow, 'arrow 16'])
    const golem = await spawn('iron_golem', 3.5)
    await shoot(golem, 25)
    await h.waitFor(() => health(golem, 100) < 100, 3000)
    await h.ticks(5)
    const dmg = r3(100 - health(golem, 100))
    const [lo, hi] = label === 'bow_full' ? [6, 11] : [14, 25]
    h.result(label, `damage=${dmg >= lo && dmg <= hi ? `${lo}..${hi}` : dmg} arrows_left=${h.inv().arrow || 0}`)
    await stand()
  }
  await clearMobs()
  await give(['bow[enchantments={infinity:1}]', 'arrow'])
  await shoot(await spawn('iron_golem', 3.5), 25)
  await h.ticks(10)
  h.result('bow_infinity.arrows_left', h.inv().arrow || 0)

  // Arrows lying on the ground: pickup 0 never, 1 by survival players, 2 only in creative.
  for (const pickup of [0, 1, 2]) {
    await clearMobs()
    await give(['stick'])
    await stand()
    h.cmd(`summon arrow ${X + 4.5} ${Y + 1.5} ${Z + 0.5} {pickup:${pickup}b}`)
    await h.ticks(30)
    await h.tp(X + 4.5, Y + 1, Z + 0.5)
    await h.ticks(20)
    h.result(`arrow_pickup.${pickup}`, `${h.inv().arrow || 0} on_ground=${named('arrow').length}`)
  }
  await stand()

  // Snowballs and eggs push a villager without hurting it.
  for (const item of ['snowball', 'egg']) {
    await clearMobs()
    await give([`${item} 4`])
    const v = await spawn('villager', 3)
    delete velocities[v.id]
    await bot.lookAt(v.position.offset(0, 1.2, 0), true)
    bot.activateItem()
    await h.waitFor(() => velocities[v.id], 3000)
    await h.ticks(5)
    // Aim drifts a little between runs; one decimal keeps the direction and strength.
    const kb = velocities[v.id] ? velocities[v.id].split(',').map(n => (Math.round(Number(n) * 10) / 10).toFixed(1)).join(',') : 'none'
    h.result(`thrown.${item}`, `kb=${kb} health=${health(v, 20)} left=${h.inv()[item] || 0}`)
  }

  // Trident thrown at a golem; with Loyalty it comes back.
  for (const t of ['trident', 'trident[enchantments={loyalty:3}]']) {
    await clearMobs()
    await give([t])
    const golem = await spawn('iron_golem', 4)
    await shoot(golem, 12)
    await h.waitFor(() => health(golem, 100) < 100, 3000)
    await h.ticks(5)
    const dmg = r3(100 - health(golem, 100))
    await h.waitFor(() => h.inv().trident, 8000)
    h.result(`throw.${t}`, `damage=${dmg} back=${!!h.inv().trident} durability=${bot.inventory.items()[0] ? bot.inventory.items()[0].durabilityUsed : '-'}`)
  }

  // Channeling calls lightning onto the mob it hits, but only in a thunderstorm.
  let bolts = 0
  bot.on('entitySpawn', (e) => { if (String(e.name).includes('lightning')) bolts++ })
  for (const weather of ['clear', 'thunder']) {
    await clearMobs()
    h.cmd(`weather ${weather}`)
    // The thunder level ramps up over about 100 ticks before the world counts as thundering.
    await h.ticks(weather === 'thunder' ? 140 : 5)
    await give(['trident[enchantments={channeling:1}]'])
    const golem = await spawn('iron_golem', 4)
    bolts = 0
    await shoot(golem, 12)
    await h.waitFor(() => health(golem, 100) < 100, 3000)
    await h.ticks(10)
    h.result(`channeling.${weather}`, `bolts=${bolts} hit=${health(golem, 100) < 100}`)
  }
  h.cmd('weather clear')

  // Riptide in water: the spin shows on the thrower, and hits an armor stand it touches. The bot
  // doesn't move itself, so the stand is placed where it already touches it. A hit on the first
  // tick ends the spin before vanilla syncs it, so the spin itself is checked without a stand.
  let spun = false
  const standEvents = []
  let armorStand
  bot._client.on('entity_metadata', (p) => {
    if (p.entityId === bot.entity.id && p.metadata.some(m => m.key === 8 && (m.value & 4))) spun = true
  })
  bot._client.on('entity_status', (p) => { if (armorStand && p.entityId === armorStand.id) standEvents.push(p.entityStatus) })
  for (const touching of [false, true]) {
    await clearMobs()
    await stand()
    h.cmd(`fill ${X - 1} ${Y + 1} ${Z - 1} ${X + 1} ${Y + 1} ${Z + 1} water`)
    await give(['trident[enchantments={riptide:1}]'])
    armorStand = undefined
    if (touching) {
      h.cmd(`summon armor_stand ${X + 0.5} ${Y + 1} ${Z + 0.6} {NoGravity:1b}`)
      await h.waitFor(() => (armorStand = named('armor_stand')[0]), 5000)
    }
    spun = false
    standEvents.length = 0
    await bot.look(0, 0, true)
    bot.activateItem()
    await h.ticks(12)
    bot.deactivateItem()
    await h.ticks(10)
    const riptideTrident = bot.inventory.items()[0]
    const durability = riptideTrident ? riptideTrident.durabilityUsed : '-'
    h.result(`riptide.${touching ? 'touching_stand' : 'alone'}`, touching
      ? `stand_events=${standEvents.join(',') || 'none'} durability=${durability}`
      : `spun=${spun} durability=${durability}`)
    h.cmd(`fill ${X - 1} ${Y + 1} ${Z - 1} ${X + 1} ${Y + 1} ${Z + 1} air`)
    await h.ticks(5)
  }

  // Crossbow charge time and the arrows a shot spawns.
  for (const c of ['crossbow', 'crossbow[enchantments={quick_charge:3}]', 'crossbow[enchantments={multishot:1}]']) {
    await clearMobs()
    await stand()
    await give([c, 'arrow 8'])
    const start = tick
    // Loading takes the arrows out of the inventory.
    const charged = () => (h.inv().arrow || 0) < 8
    bot.activateItem()
    const ok = await h.waitFor(charged, 4000)
    const took = tick - start
    bot.deactivateItem()
    await h.ticks(5)
    const before = new Set(named('arrow').map(e => e.id))
    await bot.look(0, 0, true)
    bot.activateItem()
    await h.ticks(3)
    bot.deactivateItem()
    await h.ticks(5)
    const fired = named('arrow').filter(e => !before.has(e.id)).length
    h.result(`crossbow.${c}`, `charged=${ok} ticks~${Math.round(took / 5) * 5} fired=${fired} arrows_left=${h.inv().arrow || 0}`)
  }
})
