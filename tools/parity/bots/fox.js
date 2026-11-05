// Foxes in a roofed glass pen: one picks up a dropped stack (one item into its mouth, the rest
// left on the ground), one eats the sweet berries it carries, one picks a ripe sweet berry bush,
// and one naps at noon. The bot stays in creative, so it doesn't alert the foxes.
const { make } = require('./survival-lib')
const h = make('Keeper')
const { bot } = h
const X = 9400; const Y = 200; const Z = 0
// Fox entity data 19 holds the flags; 32 is sleeping.
const FLAGS = 19; const SLEEPING = 32

function fox () {
  return bot.nearestEntity((e) => e.name === 'fox')
}

function held () {
  const f = fox()
  if (!f) return 'no fox'
  return f.equipment && f.equipment[0] ? `${f.equipment[0].name}*${f.equipment[0].count}` : 'empty'
}

function groundItems () {
  const pen = new h.Vec3(X + 0.5, Y + 2, Z + 0.5)
  const items = Object.values(bot.entities).filter((e) => e.name === 'item' && e.position.distanceTo(pen) < 4)
  return items.map((e) => {
    const it = e.getDroppedItem && e.getDroppedItem()
    return it ? `${it.name}*${it.count}` : 'unknown'
  }).sort().join(',') || 'none'
}

async function reset (time) {
  h.cmd('kill @e[type=fox]')
  h.cmd('kill @e[type=item]')
  h.cmd(`time set ${time}`)
  h.cmd(`fill ${X - 1} ${Y + 2} ${Z - 1} ${X + 1} ${Y + 2} ${Z + 1} air`)
  await h.ticks(5)
}

async function summon (extra = '') {
  h.cmd(`summon fox ${X + 0.5} ${Y + 2} ${Z + 0.5} {PersistenceRequired:1b${extra}}`)
  await h.waitFor(fox, 5000)
  await h.ticks(5)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('weather clear')
  h.cmd('gamerule mob_griefing true')
  h.cmd('gamerule advance_time false')
  h.cmd('gamerule random_tick_speed 0')
  // A 3x3 room on glass with glass walls and a stone roof: light gets in, the sky doesn't.
  h.cmd(`fill ${X - 2} ${Y + 1} ${Z - 2} ${X + 2} ${Y + 3} ${Z + 2} glass hollow`)
  h.cmd(`fill ${X - 2} ${Y + 3} ${Z - 2} ${X + 2} ${Y + 3} ${Z + 2} stone`)
  h.cmd(`tp Keeper ${X + 4.5} ${Y + 1} ${Z + 0.5}`)
  h.cmd('gamemode creative Keeper')
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  await h.ticks(10)

  // Night, so nobody sleeps through the item tests.
  await reset(18000)
  // Summoning with NBT loads CanPickUpLoot as false, as on any mob.
  await summon(',CanPickUpLoot:1b')
  h.cmd(`summon item ${X + 0.5} ${Y + 2} ${Z + 0.5} {Item:{id:"minecraft:feather",count:3},PickupDelay:0}`)
  await h.ticks(40)
  h.result('pickup.held', held())
  h.result('pickup.ground', groundItems())
  // What a fox picked up always drops when it dies.
  h.cmd('kill @e[type=item]')
  await h.ticks(5)
  h.cmd('kill @e[type=fox]')
  await h.ticks(20)
  h.result('death.ground', groundItems())

  await reset(18000)
  await summon(',equipment:{mainhand:{id:"minecraft:sweet_berries",count:1}}')
  await h.ticks(20)
  h.result('eat.before', held())
  await h.serverTicks(400)
  h.result('eat.at_400', held())
  await h.serverTicks(300)
  h.result('eat.at_700', held())

  await reset(18000)
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z + 1} grass_block`)
  h.cmd(`setblock ${X + 1} ${Y + 2} ${Z + 1} sweet_berry_bush[age=3]`)
  await h.ticks(5)
  await summon()
  await h.serverTicks(300)
  h.result('berries.bush', JSON.stringify(h.block(X + 1, Y + 2, Z + 1).getProperties()))
  h.result('berries.held', held())
  h.result('berries.ground_kinds', [...new Set(groundItems().split(',').map((s) => s.split('*')[0]))].join(','))
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z + 1} glass`)

  await reset(6000)
  await summon()
  const slept = await h.waitFor(() => ((fox().metadata[FLAGS] ?? 0) & SLEEPING) !== 0, 60000)
  h.result('noon.sleeps', slept)
  h.cmd('time set 18000')
  const woke = await h.waitFor(() => ((fox().metadata[FLAGS] ?? 0) & SLEEPING) === 0, 30000)
  h.result('night.wakes', woke)
  h.cmd('gamerule advance_time true')
  h.cmd('gamerule random_tick_speed 3')
  h.cmd('kill @e[type=fox]')
})
