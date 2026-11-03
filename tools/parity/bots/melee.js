// Player melee against mobs: damage per weapon at full charge, the attack cooldown, critical hits,
// Sharpness and Strength, sweep attacks, knockback (base, sprint, Knockback enchantment) and the
// weapon durability each hit costs. Targets are NoAI mobs; their health comes from entity metadata
// and knockback from the velocity packets the bot receives.
const { make } = require('./survival-lib')
const h = make('Fighter')
const { bot } = h
const X = 1700; const Y = 200; const Z = 0
const HEALTH = 9

const velocities = {}
bot._client.on('entity_velocity', (p) => {
  const v = p.velocity
  velocities[p.entityId] = `${v.x.toFixed(3)},${v.y.toFixed(3)},${v.z.toFixed(3)}`
})

const r3 = (n) => Math.round(n * 1000) / 1000
const near = (name, x, z) => Object.values(bot.entities).find(e => e.name === name &&
  Math.abs(e.position.x - x) < 0.6 && Math.abs(e.position.z - z) < 0.6)
const health = (e, max) => (e.metadata && typeof e.metadata[HEALTH] === 'number') ? e.metadata[HEALTH] : max

async function clearMobs () {
  h.cmd('tp @e[type=!player] ~ -300 ~')
  h.cmd('kill @e[type=!player]')
  await h.ticks(5)
}

async function spawn (type, dx, dz, extra = '') {
  const x = X + 0.5 + dx; const z = Z + 0.5 + dz
  // Decimal coordinates: commands centre whole numbers on the block.
  h.cmd(`summon ${type} ${x.toFixed(3)} ${Y + 1} ${z.toFixed(3)} {NoAI:1b,Silent:1b,PersistenceRequired:1b,Tags:["t"]${extra}}`)
  let e
  await h.waitFor(() => (e = near(type, x, z)), 5000)
  await h.ticks(2)
  return e
}

async function hold (item) {
  h.cmd(`clear ${bot.username}`)
  h.cmd(`item replace entity ${bot.username} hotbar.0 with ${item}`)
  bot.setQuickBarSlot(0)
  const name = item.split('[')[0].replace('minecraft:', '')
  await h.waitFor(() => bot.heldItem && bot.heldItem.name === name, 5000)
  // Changing the held item resets the attack strength ticker; let it charge fully.
  await h.ticks(40)
}

async function stand () {
  bot.clearControlStates()
  await h.tp(X + 0.5, Y + 1, Z + 0.5, 0, 0)
  await h.ticks(10)
}

// Hits `e` and returns how much health it lost.
async function hit (e, max) {
  const before = health(e, max)
  bot.attack(e)
  await h.waitFor(() => health(e, max) < before, 2000)
  await h.ticks(2)
  return r3(before - health(e, max))
}

async function waitFalling () {
  bot.setControlState('jump', true)
  await h.waitFor(() => !bot.entity.onGround, 2000)
  bot.setControlState('jump', false)
  await h.waitFor(() => bot.entity.velocity.y < -0.1, 2000)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('difficulty peaceful')
  h.cmd('gamerule mob_drops false')
  await stand()

  const weapons = ['air', 'wooden_sword', 'stone_sword', 'iron_sword', 'diamond_sword', 'netherite_sword',
    'iron_axe', 'netherite_axe', 'iron_pickaxe', 'iron_shovel', 'iron_hoe', 'trident', 'mace', 'stick']
  for (const w of weapons) {
    await clearMobs()
    if (w === 'air') { h.cmd(`clear ${bot.username}`); await h.ticks(40) } else await hold(w)
    const golem = await spawn('iron_golem', 0, 2.5)
    h.result(`full.${w}`, await hit(golem, 100))
    await h.ticks(5)
    h.result(`durability.${w}`, bot.heldItem ? bot.heldItem.durabilityUsed : 'none')
  }

  // Attack cooldown: the second of two hits sent in the same tick lands with almost no charge.
  for (const w of ['diamond_sword', 'netherite_axe']) {
    await clearMobs()
    await hold(w)
    const a = await spawn('iron_golem', -1.5, 2.5)
    const b = await spawn('iron_golem', 1.5, 2.5)
    const hb = health(b, 100)
    bot.attack(a)
    bot.attack(b)
    await h.waitFor(() => health(b, 100) < hb, 2000)
    await h.ticks(2)
    h.result(`uncharged.${w}`, r3(100 - health(a, 100)) + '/' + r3(hb - health(b, 100)))
  }

  // Swinging at air and switching items both start the charge over.
  {
    await clearMobs()
    await hold('diamond_sword')
    const golem = await spawn('iron_golem', 0, 2.5)
    bot.swingArm()
    bot.attack(golem)
    await h.waitFor(() => health(golem, 100) < 100, 2000)
    await h.ticks(2)
    h.result('uncharged.after_air_swing', r3(100 - health(golem, 100)))
    await clearMobs()
    h.cmd(`item replace entity ${bot.username} hotbar.1 with netherite_axe`)
    await h.waitFor(() => bot.inventory.slots[37], 5000)
    await h.ticks(40)
    const golem2 = await spawn('iron_golem', 0, 2.5)
    bot.setQuickBarSlot(1)
    // A few ticks so the switch is seen by a server tick first; the exact charge then depends
    // on timing, so only whether the hit was weak is compared.
    await h.ticks(3)
    bot.attack(golem2)
    await h.waitFor(() => health(golem2, 100) < 100, 2000)
    await h.ticks(2)
    const switched = 100 - health(golem2, 100)
    h.result('uncharged.after_switch', switched < 5 ? 'weak' : r3(switched))
  }

  // Critical hits, with and without Sharpness.
  for (const w of ['diamond_sword', 'diamond_sword[enchantments={sharpness:5}]', 'air']) {
    await clearMobs()
    if (w === 'air') { h.cmd(`clear ${bot.username}`); await h.ticks(40) } else await hold(w)
    const golem = await spawn('iron_golem', 0, 2.5)
    await waitFalling()
    h.result(`crit.${w}`, await hit(golem, 100))
    await h.waitFor(() => bot.entity.onGround, 3000)
    await stand()
  }
  await clearMobs()
  await hold('diamond_sword[enchantments={sharpness:5}]')
  h.result('full.sharpness5', await hit(await spawn('iron_golem', 0, 2.5), 100))

  // Strength scales with the attack cooldown in vanilla since it's an attack damage modifier.
  h.cmd(`effect give ${bot.username} strength 60 0`)
  await clearMobs()
  await hold('diamond_sword')
  h.result('full.strength', await hit(await spawn('iron_golem', 0, 2.5), 100))
  {
    await clearMobs()
    const a = await spawn('iron_golem', -1.5, 2.5)
    const b = await spawn('iron_golem', 1.5, 2.5)
    await h.ticks(40)
    bot.attack(a)
    bot.attack(b)
    await h.waitFor(() => health(b, 100) < 100, 2000)
    await h.ticks(2)
    h.result('uncharged.strength', r3(100 - health(b, 100)))
  }
  h.cmd(`effect clear ${bot.username}`)
  h.cmd(`effect give ${bot.username} weakness 60 0`)
  await clearMobs()
  await hold('netherite_axe')
  h.result('full.weakness', await hit(await spawn('iron_golem', 0, 2.5), 100))
  h.cmd(`effect clear ${bot.username}`)

  // The held item's attack damage goes away with the item.
  await clearMobs()
  await hold('diamond_sword')
  h.cmd(`clear ${bot.username}`)
  await h.ticks(40)
  h.result('full.fist_after_clear', await hit(await spawn('iron_golem', 0, 2.5), 100))

  // Sweep attacks hit mobs next to the target for 1 + ratio * damage and push them 0.4.
  for (const w of ['diamond_sword', 'diamond_sword[enchantments={sweeping_edge:3}]', 'diamond_axe']) {
    await clearMobs()
    await hold(w)
    const target = await spawn('villager', 0, 2)
    const side = await spawn('villager', 0.8, 2)
    delete velocities[side.id]
    await hit(target, 20)
    await h.ticks(5)
    h.result(`sweep.${w}`, `${r3(20 - health(side, 20))} kb=${velocities[side.id] || 'none'}`)
  }

  // Knockback on a villager (no knockback resistance) straight ahead.
  const kb = async (label, item, sprint) => {
    await clearMobs()
    if (item === 'air') { h.cmd(`clear ${bot.username}`); await h.ticks(40) } else await hold(item)
    const v = await spawn('villager', 0, 2)
    delete velocities[v.id]
    if (sprint) { bot.setControlState('sprint', true); await h.ticks(2) }
    await hit(v, 20)
    await h.ticks(3)
    bot.setControlState('sprint', false)
    h.result(`kb.${label}`, velocities[v.id] || 'none')
    await stand()
  }
  await kb('fist', 'air', false)
  await kb('sprint_fist', 'air', true)
  await kb('knockback2', 'stick[enchantments={knockback:2}]', false)
  await kb('sprint_knockback1', 'stick[enchantments={knockback:1}]', true)
  // The killing blow still knocks the mob back.
  {
    await clearMobs()
    h.cmd(`clear ${bot.username}`)
    await h.ticks(40)
    const v = await spawn('villager', 0, 2, ',Health:1f')
    delete velocities[v.id]
    bot.attack(v)
    await h.waitFor(() => velocities[v.id], 2000)
    h.result('kb.killing_blow', velocities[v.id] || 'none')
  }

  // Armor on the target: a zombie's natural 2 points, and worn armor with toughness. Peaceful
  // would remove the zombies.
  h.cmd('difficulty easy')
  h.cmd('time set midnight')
  for (const [label, extra] of [['zombie', ''],
    ['zombie_netherite_chest', ',equipment:{chest:{id:"minecraft:netherite_chestplate",count:1}}']]) {
    await clearMobs()
    await hold('diamond_sword')
    h.result(`armored.${label}`, await hit(await spawn('zombie', 0, 2, extra), 20))
  }

  // Invulnerability frames: a second hit within 10 ticks only deals the amount above the first.
  for (const extra of [2, 10]) {
    await clearMobs()
    await hold('wooden_sword')
    const golem = await spawn('iron_golem', 0, 2.5)
    await hit(golem, 100)
    h.cmd(`damage @e[tag=t,limit=1] ${extra} minecraft:generic`)
    await h.ticks(15)
    h.result(`iframes.4_then_${extra}`, r3(100 - health(golem, 100)))
  }
})
