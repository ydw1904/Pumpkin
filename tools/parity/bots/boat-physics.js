// Boat physics (vanilla AbstractBoat.tick on the server): a boat summoned in the air falls, one
// dropped into a pool floats at the water level, flowing water carries an empty boat, a pig that
// bumps into an empty boat gets in while an iron golem only pushes it, an upward bubble column
// launches a boat and a downward one sinks it and throws its passenger out, and a boat under
// water ejects its passenger after 60 ticks.
const { make } = require('./survival-lib')
const h = make('Boater')
const { bot } = h
const X = 3800; const Y = 200; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m.toString()))

async function ask (cmd) {
  const c0 = chat.length
  bot.chat(cmd)
  if (!await h.waitFor(() => chat.length > c0, 5000)) return '<no reply>'
  await h.sleep(30)
  return chat[c0]
}

// The server's position, rounded so float noise doesn't count.
async function pos (selector, digits = 3) {
  const reply = await ask(`/data get entity ${selector} Pos`)
  const nums = reply.replace(/^.*data: /, '').match(/-?\d+(\.\d+)?(E-?\d+)?/g)
  return nums ? nums.map((n) => Number(n)).map((n) => Number(n.toFixed(digits))) : null
}

const boats = () => Object.values(bot.entities).filter((e) => e.name === 'oak_boat')
const named = (name) => Object.values(bot.entities).filter((e) => e.name === name)

// Whether the pig rides the boat, asked on the server.
async function aboard () {
  return (await ask('/execute as @e[type=pig,limit=1] on vehicle if entity @s[type=oak_boat]')).includes('passed')
}

async function ejectedWithin (ms) {
  const end = Date.now() + ms
  while (Date.now() < end) {
    if (!await aboard()) return true
    await h.sleep(250)
  }
  return false
}

async function clearBoats () {
  h.cmd('kill @e[type=oak_boat]')
  h.cmd('kill @e[type=pig]')
  h.cmd('kill @e[type=iron_golem]')
  h.cmd('kill @e[type=item]')
  await h.waitFor(() => boats().length === 0, 5000)
  await h.sleep(500)
}

// A stone basin with two layers of water sources, `inside` blocks wide.
function pool (x0, z0, inside) {
  h.cmd(`fill ${x0} ${Y + 1} ${z0} ${x0 + inside + 1} ${Y + 3} ${z0 + inside + 1} stone`)
  h.cmd(`fill ${x0 + 1} ${Y + 2} ${z0 + 1} ${x0 + inside} ${Y + 3} ${z0 + inside} water`)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('gamemode creative Boater')
  await h.tp(X + 0.5, Y + 1, Z + 7.5)
  await h.sleep(500)

  // In the air above the stone floor.
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 6} ${Z + 0.5}`)
  await h.waitFor(() => boats().length === 1, 5000)
  await h.sleep(500)
  const falling = await pos('@e[type=oak_boat,limit=1]')
  h.result('air.falling', falling !== null && falling[1] < Y + 5.9)
  await h.sleep(3000)
  h.result('air.rest', (await pos('@e[type=oak_boat,limit=1]')).join(','))
  await clearBoats()

  // Dropped into a pool: it lands on the surface and floats there.
  pool(X - 7, Z - 7, 5)
  await h.sleep(1500)
  h.cmd(`summon oak_boat ${X - 4} ${Y + 7} ${Z - 4}`)
  await h.waitFor(() => boats().length === 1, 5000)
  await h.sleep(5000)
  const floating = await pos('@e[type=oak_boat,limit=1]')
  h.result('pool.rest', floating.join(','))
  const surface = floating[1]

  // An upward bubble column under it launches it after 60 ticks over it.
  h.cmd(`setblock ${X - 4} ${Y + 1} ${Z - 4} soul_sand`)
  let highest = surface
  const launchEnd = Date.now() + 8000
  while (Date.now() < launchEnd) {
    const boat = boats()[0]
    if (boat) highest = Math.max(highest, boat.position.y)
    await h.sleep(50)
  }
  h.result('bubble_up.launched', highest - surface > 2)
  await clearBoats()

  // A downward column sinks a boat with a pig in it and throws the pig out.
  h.cmd(`setblock ${X - 4} ${Y + 1} ${Z - 4} magma_block`)
  await h.sleep(1500)
  h.cmd(`summon oak_boat ${X - 4} ${Y + 4} ${Z - 4} {Passengers:[{id:"minecraft:pig",NoAI:1b}]}`)
  await h.waitFor(() => boats().length === 1 && named('pig').length === 1, 5000)
  await h.sleep(500)
  h.result('bubble_down.pig_aboard', await aboard())
  h.result('bubble_down.pig_ejected', await ejectedWithin(8000))
  await clearBoats()

  // Held under water by a stone roof: the passenger falls out after 60 ticks.
  h.cmd(`setblock ${X - 4} ${Y + 1} ${Z - 4} stone`)
  h.cmd(`fill ${X - 6} ${Y + 4} ${Z - 6} ${X - 2} ${Y + 4} ${Z - 2} stone`)
  await h.sleep(1500)
  h.cmd(`summon oak_boat ${X - 4} ${Y + 2} ${Z - 4} {Passengers:[{id:"minecraft:pig",NoAI:1b}]}`)
  await h.waitFor(() => boats().length === 1 && named('pig').length === 1, 5000)
  await h.sleep(500)
  h.result('underwater.pig_aboard', await aboard())
  h.result('underwater.pig_ejected', await ejectedWithin(8000))
  await clearBoats()

  // A three wide channel fed from its west end: the water flows east and carries the boat to
  // the wall at its east end.
  h.cmd(`fill ${X + 1} ${Y + 1} ${Z - 6} ${X + 8} ${Y + 1} ${Z - 2} stone`)
  h.cmd(`fill ${X + 2} ${Y + 1} ${Z - 5} ${X + 7} ${Y + 1} ${Z - 3} air`)
  h.cmd(`fill ${X + 2} ${Y + 1} ${Z - 5} ${X + 2} ${Y + 1} ${Z - 3} water`)
  await h.sleep(3000)
  h.cmd(`summon oak_boat ${X + 3.5} ${Y + 1.1} ${Z - 4}`)
  await h.waitFor(() => boats().length === 1, 5000)
  await h.sleep(500)
  const start = await pos('@e[type=oak_boat,limit=1]')
  await h.sleep(3000)
  const drifted = await pos('@e[type=oak_boat,limit=1]')
  h.result('flow.downstream', drifted[0] - start[0] > 0.5)
  h.result('flow.stays_in_channel', Math.abs(drifted[2] - (Z - 4)) < 1)
  await h.sleep(7000)
  h.result('flow.end_block', Math.floor((await pos('@e[type=oak_boat,limit=1]'))[0]) - X)
  await clearBoats()

  // On the floor: a pig next to an empty boat gets in, an iron golem is too wide and pushes it.
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 1} ${Z + 3.5}`)
  await h.waitFor(() => boats().length === 1, 5000)
  await h.sleep(500)
  h.cmd(`summon pig ${X + 1.5} ${Y + 1} ${Z + 3.5} {NoAI:1b}`)
  await h.waitFor(() => named('pig')[0] && named('pig')[0].vehicle, 3000)
  h.result('pickup.pig', !!(named('pig')[0] && named('pig')[0].vehicle && named('pig')[0].vehicle.name))
  await clearBoats()
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 1} ${Z + 3.5}`)
  await h.waitFor(() => boats().length === 1, 5000)
  await h.sleep(500)
  const before = await pos('@e[type=oak_boat,limit=1]')
  h.cmd(`summon iron_golem ${X + 1.6} ${Y + 1} ${Z + 3.5} {NoAI:1b}`)
  await h.sleep(1500)
  const golem = named('iron_golem')[0]
  h.result('pickup.golem_rides', !!(golem && golem.vehicle))
  const after = await pos('@e[type=oak_boat,limit=1]')
  h.result('push.golem_pushes_boat_west', after[0] < before[0] - 0.01)
  await clearBoats()

  // Broken in survival, a named boat drops itself with the name.
  h.cmd('gamemode survival Boater')
  await h.clear()
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 1} ${Z + 5.5} {CustomName:"Dinghy"}`)
  await h.waitFor(() => boats().length === 1, 5000)
  await h.sleep(500)
  // mineflayer's attack writes an old packet layout, so the hit comes from /damage.
  h.cmd('damage @e[type=oak_boat,limit=1] 5 minecraft:player_attack by Boater')
  await h.waitFor(() => boats().length === 0, 3000)
  h.result('break.broken', boats().length === 0)
  h.cmd(`tp Boater ${X + 0.5} ${Y + 1} ${Z + 5.5}`)
  await h.waitFor(() => (h.inv().oak_boat || 0) > 0, 5000)
  const dropped = bot.inventory.items().find((it) => it.name === 'oak_boat')
  h.result('break.drop', dropped ? `${dropped.name}x${dropped.count}` : 'none')
  h.result('break.named', !!dropped && JSON.stringify(dropped.customName ?? null).includes('Dinghy'))
})
