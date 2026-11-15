// Vibrations a player causes: sculk sensors activated by walking, a placed block and an opened
// chest, nothing for sneaking or through wool, calibrated sensors filtering by frequency, the
// comparator reading the frequency, shriekers answering a sensor, everyday actions (eating,
// falling, splashing, throwing, equipping, riding) read through the comparator, and a catalyst
// spreading sculk when a mob dies next to it.
const { make } = require('./survival-lib')
const h = make('Listener')
const { bot, Vec3 } = h
const X = 7000; const Y = 200; const Z = 0
// Scenes sit further apart than a calibrated sensor (16 blocks) can hear.
const SPACING = 40
// A sensor is back to inactive 40 ticks after it activates.
const RESET_TICKS = 60

const props = (x, y, z) => { const b = h.block(x, y, z); return b ? b.getProperties() : {} }
// Watches a block for the values one of its properties passes through.
function watch (x, y, z, key) {
  const seen = []
  const fn = (oldB, newB) => {
    if (newB && newB.position.x === x && newB.position.y === y && newB.position.z === z) {
      const v = key ? newB.getProperties()[key] : newB.name
      if (seen[seen.length - 1] !== v) seen.push(v)
    }
  }
  bot.on('blockUpdate', fn)
  return { stop: () => { bot.removeListener('blockUpdate', fn); return seen.join('>') || 'none' } }
}

// A scene's floor, with the bot standing on it at `standX` and everything settled.
async function scene (i, standX, standZ = Z) {
  const x = X + i * SPACING
  await h.arena(x, Y, Z, 10)
  await h.tp(standX(x) + 0.5, Y + 1, standZ + 0.5)
  await h.serverTicks(20)
  return x
}

// A sensor at `x` with a comparator reading it from the east and a wire after that.
function sensorWithComparator (x, block = 'sculk_sensor') {
  h.cmd(`setblock ${x} ${Y + 1} ${Z} ${block}`)
  h.cmd(`setblock ${x + 1} ${Y + 1} ${Z} comparator[facing=west]`)
  h.cmd(`setblock ${x + 2} ${Y + 1} ${Z} redstone_wire`)
}

async function placeStone (x, z) {
  await h.give('stone', 1)
  await bot.equip(bot.inventory.items().find(it => it.name === 'stone'), 'hand')
  await bot.placeBlock(h.block(x, Y, z), new Vec3(0, 1, 0)).catch((e) => h.result('place_error', e.message.split('\n')[0]))
}

h.start(async () => {
  h.cmd('time set 18000')
  h.cmd('gamerule advance_time false')
  h.cmd('weather clear')
  h.cmd('difficulty normal')
  h.cmd(`effect give ${bot.username} resistance infinite 4 true`)

  // Walking up to a sensor.
  let x = await scene(0, (sx) => sx - 6)
  sensorWithComparator(x)
  await h.serverTicks(RESET_TICKS)
  let phase = watch(x, Y + 1, Z, 'sculk_sensor_phase')
  let wire = watch(x + 2, Y + 1, Z, 'power')
  await bot.lookAt(new Vec3(x + 0.5, Y + 1.6, Z + 0.5), true)
  bot.setControlState('forward', true)
  await h.ticks(12)
  bot.setControlState('forward', false)
  await h.serverTicks(RESET_TICKS)
  h.result('walk_phase', phase.stop())
  h.result('walk_activated', wire.stop() !== 'none')

  // Sneaking up to one: nothing.
  x = await scene(1, (sx) => sx - 6)
  sensorWithComparator(x)
  bot.setControlState('sneak', true)
  await h.serverTicks(RESET_TICKS)
  phase = watch(x, Y + 1, Z, 'sculk_sensor_phase')
  await bot.lookAt(new Vec3(x + 0.5, Y + 1.6, Z + 0.5), true)
  bot.setControlState('forward', true)
  await h.ticks(30)
  bot.setControlState('forward', false)
  await h.serverTicks(40)
  bot.setControlState('sneak', false)
  h.result('sneak_phase', phase.stop())

  // A placed block two blocks away: frequency 13, power from the distance.
  x = await scene(2, (sx) => sx - 5)
  sensorWithComparator(x)
  await h.serverTicks(RESET_TICKS)
  phase = watch(x, Y + 1, Z, 'sculk_sensor_phase')
  let power = watch(x, Y + 1, Z, 'power')
  wire = watch(x + 2, Y + 1, Z, 'power')
  await placeStone(x - 2, Z)
  await h.serverTicks(RESET_TICKS)
  h.result('place_phase', phase.stop())
  h.result('place_power', power.stop())
  h.result('place_comparator', wire.stop())

  // An opened and closed chest: frequency 10, then 9.
  x = await scene(3, (sx) => sx - 5)
  sensorWithComparator(x)
  h.cmd(`setblock ${x - 3} ${Y + 1} ${Z} chest[facing=west]`)
  await h.serverTicks(RESET_TICKS)
  power = watch(x, Y + 1, Z, 'power')
  wire = watch(x + 2, Y + 1, Z, 'power')
  const chest = await bot.openContainer(h.block(x - 3, Y + 1, Z))
  await h.serverTicks(RESET_TICKS)
  chest.close()
  await h.serverTicks(RESET_TICKS)
  h.result('chest_power', power.stop())
  h.result('chest_comparator', wire.stop())

  // The same chest behind a wall of wool: occluded.
  x = await scene(4, (sx) => sx - 5)
  sensorWithComparator(x)
  h.cmd(`setblock ${x - 3} ${Y + 1} ${Z} chest[facing=west]`)
  h.cmd(`fill ${x - 1} ${Y + 1} ${Z - 1} ${x - 1} ${Y + 3} ${Z + 1} white_wool`)
  await h.serverTicks(RESET_TICKS)
  phase = watch(x, Y + 1, Z, 'sculk_sensor_phase')
  const walled = await bot.openContainer(h.block(x - 3, Y + 1, Z))
  await h.serverTicks(20)
  walled.close()
  await h.serverTicks(40)
  h.result('wool_phase', phase.stop())

  // Calibrated sensors listening for frequency 10, from a wire at their back: a chest opens
  // one, a placed block (13) doesn't open the other.
  for (const [i, name] of [[5, 'calibrated_chest'], [6, 'calibrated_place']]) {
    x = await scene(i, (sx) => sx - 3, Z + 3)
    h.cmd(`setblock ${x} ${Y + 1} ${Z} calibrated_sculk_sensor[facing=east]`)
    h.cmd(`setblock ${x} ${Y + 1} ${Z + 1} comparator[facing=north]`)
    h.cmd(`setblock ${x} ${Y + 1} ${Z + 2} redstone_wire`)
    // Six wires from a redstone block: the last one carries 10.
    h.cmd(`setblock ${x - 7} ${Y + 1} ${Z} redstone_block`)
    h.cmd(`fill ${x - 6} ${Y + 1} ${Z} ${x - 1} ${Y + 1} ${Z} redstone_wire`)
    h.cmd(`setblock ${x - 3} ${Y + 1} ${Z - 2} chest`)
    await h.serverTicks(RESET_TICKS)
    h.result(`${name}_input`, props(x - 1, Y + 1, Z).power)
    power = watch(x, Y + 1, Z, 'power')
    wire = watch(x, Y + 1, Z + 2, 'power')
    if (name === 'calibrated_chest') {
      const c = await bot.openContainer(h.block(x - 3, Y + 1, Z - 2))
      await h.serverTicks(30)
      c.close()
    } else {
      await placeStone(x - 1, Z + 4)
    }
    await h.serverTicks(RESET_TICKS)
    h.result(`${name}_power`, power.stop())
    h.result(`${name}_comparator`, wire.stop())
  }

  // A sensor a player sets off makes a shrieker next to it shriek. One that can summon warns:
  // darkness for the player once the shriek ends.
  let darkness = 0
  bot._client.on('entity_effect', (p) => {
    const effect = bot.registry.effects[p.effectId]
    if (p.entityId === bot.entity.id && effect && /darkness/i.test(effect.name)) darkness++
  })
  for (const [i, summon] of [[7, false], [8, true]]) {
    x = await scene(i, (sx) => sx - 5)
    h.cmd(`effect clear ${bot.username} darkness`)
    h.cmd(`setblock ${x} ${Y + 1} ${Z} sculk_sensor`)
    h.cmd(`setblock ${x + 2} ${Y + 1} ${Z} sculk_shrieker[can_summon=${summon}]`)
    await h.serverTicks(RESET_TICKS)
    darkness = 0
    const shriek = watch(x + 2, Y + 1, Z, 'shrieking')
    await placeStone(x - 2, Z)
    await h.serverTicks(120)
    h.result(`shrieker_summon_${summon}`, shriek.stop())
    h.result(`shrieker_summon_${summon}_darkness`, darkness > 0)
  }

  // A chest a player opens next to a sensor with a shrieker: the shrieker only answers vibrations
  // that a player caused, so the chest has to name its opener.
  x = await scene(10, (sx) => sx - 5)
  h.cmd(`setblock ${x} ${Y + 1} ${Z} sculk_sensor`)
  h.cmd(`setblock ${x + 2} ${Y + 1} ${Z} sculk_shrieker`)
  h.cmd(`setblock ${x - 3} ${Y + 1} ${Z} chest[facing=west]`)
  await h.serverTicks(RESET_TICKS)
  const chestShriek = watch(x + 2, Y + 1, Z, 'shrieking')
  const shriekChest = await bot.openContainer(h.block(x - 3, Y + 1, Z))
  await h.serverTicks(20)
  shriekChest.close()
  await h.serverTicks(120)
  h.result('shrieker_chest', chestShriek.stop())

  // Everyday things a player does near a sensor, each read through the comparator: the wire
  // shows the frequency of every vibration the sensor took.
  const vibrationScenes = [
    ['trapdoor', async (x) => {
      h.cmd(`setblock ${x - 2} ${Y + 1} ${Z} oak_trapdoor`)
      await h.serverTicks(5)
      await bot.activateBlock(h.block(x - 2, Y + 1, Z))
    }],
    ['eat', async () => {
      await h.give('golden_apple', 1)
      await bot.equip(bot.inventory.items().find(it => it.name === 'golden_apple'), 'hand')
      await bot.consume().catch((e) => h.result('eat_error', e.message))
    }],
    ['drink', async () => {
      await h.give('potion[potion_contents={potion:"minecraft:water"}]', 1)
      await bot.equip(bot.inventory.items().find(it => it.name === 'potion'), 'hand')
      await bot.consume().catch((e) => h.result('drink_error', e.message))
    }],
    ['equip', async () => {
      await h.give('iron_helmet', 1)
      await bot.equip(bot.inventory.items().find(it => it.name === 'iron_helmet'), 'hand')
      bot.activateItem()
      await h.ticks(2)
      bot.deactivateItem()
    }],
    ['fall', async (x) => {
      await h.tp(x - 3 + 0.5, Y + 4, Z + 0.5)
    }],
    ['splash', async (x) => {
      h.cmd(`fill ${x - 4} ${Y + 1} ${Z - 1} ${x - 3} ${Y + 1} ${Z + 1} water`)
      await h.serverTicks(5)
      await h.tp(x - 4 + 0.5, Y + 5, Z + 0.5)
    }],
    ['toss', async () => {
      await h.give('stone', 1)
      await bot.equip(bot.inventory.items().find(it => it.name === 'stone'), 'hand')
      await bot.tossStack(bot.heldItem)
    }],
    ['snowball', async (x) => {
      await h.give('snowball', 1)
      await bot.equip(bot.inventory.items().find(it => it.name === 'snowball'), 'hand')
      await bot.lookAt(new Vec3(x - 2.5, Y + 1, Z + 0.5), true)
      bot.activateItem()
      await h.ticks(1)
      bot.deactivateItem()
    }],
    ['item_frame', async (x) => {
      h.cmd(`summon item_frame ${x - 3} ${Y + 1} ${Z} {Facing:1b}`)
      // The frame's placement settles before the sensor listens again.
      await h.serverTicks(RESET_TICKS)
      await h.give('stone', 1)
      await bot.equip(bot.inventory.items().find(it => it.name === 'stone'), 'hand')
      const frame = Object.values(bot.entities).find(e => e.name === 'item_frame')
      if (frame) await bot.activateEntity(frame)
      else h.result('item_frame_missing', true)
    }],
    ['damage', async (x) => {
      h.cmd(`summon pig ${x - 3 + 0.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,Silent:1b}`)
      await h.serverTicks(RESET_TICKS)
      h.cmd('damage @e[type=pig,limit=1] 1')
    }],
    ['minecart', async (x) => {
      h.cmd(`setblock ${x - 3} ${Y + 1} ${Z} rail`)
      h.cmd(`summon minecart ${x - 3 + 0.5} ${Y + 1} ${Z + 0.5}`)
      await h.serverTicks(RESET_TICKS)
      const cart = Object.values(bot.entities).find(e => e.name === 'minecart')
      if (cart) bot.mount(cart)
      else h.result('minecart_missing', true)
      await h.serverTicks(RESET_TICKS)
      h.cmd(`ride ${bot.username} dismount`)
    }]
  ]
  for (const [idx, [name, act]] of vibrationScenes.entries()) {
    x = await scene(11 + idx, (sx) => sx - 5, Z - 3)
    await h.clear()
    sensorWithComparator(x)
    await h.serverTicks(RESET_TICKS)
    wire = watch(x + 2, Y + 1, Z, 'power')
    await act(x)
    await h.serverTicks(RESET_TICKS * 2)
    h.result(`vibration_${name}`, wire.stop())
  }

  // A mob the player kills next to a catalyst: the catalyst blooms, takes the experience and
  // spreads sculk from where the mob died.
  x = await scene(9, (sx) => sx - 5)
  h.cmd(`setblock ${x} ${Y + 1} ${Z} sculk_catalyst`)
  h.cmd(`summon zombie ${x - 2.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,Health:1f,PersistenceRequired:1b}`)
  await h.serverTicks(40)
  const bloom = watch(x, Y + 1, Z, 'bloom')
  const zombie = Object.values(bot.entities).find(e => e.name === 'zombie')
  h.result('catalyst_zombie', !!zombie)
  // Player damage, so the zombie would drop experience without the catalyst.
  h.cmd(`damage @e[type=zombie,limit=1] 5 minecraft:player_attack by ${bot.username}`)
  await h.serverTicks(40)
  h.result('catalyst_zombie_dead', !Object.values(bot.entities).some(e => e.name === 'zombie'))
  h.result('catalyst_bloom', bloom.stop())
  h.result('catalyst_orbs', h.orbs().length)
  await h.serverTicks(200)
  let sculk = 0
  for (let dx = -6; dx <= 6; dx++) {
    for (let dz = -6; dz <= 6; dz++) {
      for (let dy = 0; dy <= 1; dy++) {
        const name = h.blockName(x + dx, Y + dy, Z + dz)
        if (name === 'sculk' || name === 'sculk_vein') sculk++
      }
    }
  }
  h.result('catalyst_spread', sculk > 0)

  // A lightning strike seven blocks away, last since it leaves fire behind.
  x = await scene(11 + vibrationScenes.length, (sx) => sx - 5, Z - 3)
  sensorWithComparator(x)
  await h.serverTicks(RESET_TICKS)
  wire = watch(x + 2, Y + 1, Z, 'power')
  h.cmd(`summon lightning_bolt ${x - 6} ${Y + 1} ${Z + 4}`)
  await h.serverTicks(RESET_TICKS * 2)
  h.result('vibration_lightning', wire.stop())
})
