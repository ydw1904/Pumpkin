// Pointed dripstone: thickness follows the column as it is built, a stalactite falls when its
// support goes (one falling block per piece, only the tip hurts, each piece drops itself and
// plays the landing sound), an unsupported stalagmite breaks, landing on a stalagmite tip hurts
// with the stalagmite damage type, and a stalactite under a dripstone block with water above grows.
// pumpkin-settings: spawn_protection=0
const { make } = require('./survival-lib')
const h = make('Dripper', 600000)
const { bot } = h
const X = 8200; const Y = 200; const Z = 0

const props = (x, y, z) => { const b = h.block(x, y, z); return b ? b.getProperties() : {} }
const thickness = (x, y, z) => h.blockName(x, y, z) === 'pointed_dripstone' ? props(x, y, z).thickness : h.blockName(x, y, z)
const levelEvents = []
bot._client.on('world_event', (p) => levelEvents.push(p.effectId))
const damage = []
bot._client.on('damage_event', (p) => { if (bot.entity && p.entityId === bot.entity.id) damage.push(p.sourceTypeId) })
const spawned = []
const fallingAt = []
bot.on('entitySpawn', (e) => {
  spawned.push(e.name)
  // Terrain far below can drop sand of its own.
  if (e.name === 'falling_block' && e.position.y > Y) fallingAt.push(`${Math.floor(e.position.x) - X},${Math.floor(e.position.y) - Y},${Math.floor(e.position.z) - Z}`)
})
const heal = async () => {
  h.cmd(`effect give ${bot.username} instant_health 1 10 true`)
  await h.waitFor(() => bot.health >= 20, 3000)
  await h.ticks(5)
}

h.start(async () => {
  h.cmd('time set 6000')
  h.cmd('weather clear')
  h.cmd('gamerule random_tick_speed 0')
  await h.arena(X, Y, Z, 8)

  // Built top down under a stone ceiling: each new piece reshapes the ones above it.
  const CX = X + 3; const CZ = Z
  h.cmd(`setblock ${CX} ${Y + 6} ${CZ} stone`)
  h.cmd(`setblock ${CX} ${Y + 5} ${CZ} pointed_dripstone[vertical_direction=down,thickness=tip]`)
  h.cmd(`setblock ${CX} ${Y + 4} ${CZ} pointed_dripstone[vertical_direction=down,thickness=tip]`)
  h.cmd(`setblock ${CX} ${Y + 3} ${CZ} pointed_dripstone[vertical_direction=down,thickness=tip]`)
  await h.waitFor(() => h.blockName(CX, Y + 3, CZ) === 'pointed_dripstone', 3000)
  await h.ticks(5)
  h.result('column', [5, 4, 3].map(dy => thickness(CX, Y + dy, CZ)).join('>'))
  // A stalagmite tip under the stalactite tip, then the stalactite tip grows down to meet it.
  h.cmd(`setblock ${CX} ${Y + 1} ${CZ} pointed_dripstone[vertical_direction=up,thickness=tip]`)
  h.cmd(`setblock ${CX} ${Y + 2} ${CZ} pointed_dripstone[vertical_direction=up,thickness=tip_merge]`)
  await h.waitFor(() => h.blockName(CX, Y + 2, CZ) === 'pointed_dripstone', 3000)
  await h.ticks(5)
  h.result('with_stalagmite', [5, 4, 3, 2, 1].map(dy => thickness(CX, Y + dy, CZ)).join('>'))
  h.cmd(`fill ${CX} ${Y + 1} ${CZ} ${CX} ${Y + 6} ${CZ} air`)
  await h.ticks(10)

  // Falling stalactite over the bot.
  const FX = X; const FZ = Z
  h.cmd(`setblock ${FX} ${Y + 6} ${FZ} stone`)
  h.cmd(`setblock ${FX} ${Y + 5} ${FZ} pointed_dripstone[vertical_direction=down,thickness=base]`)
  h.cmd(`setblock ${FX} ${Y + 4} ${FZ} pointed_dripstone[vertical_direction=down,thickness=frustum]`)
  h.cmd(`setblock ${FX} ${Y + 3} ${FZ} pointed_dripstone[vertical_direction=down,thickness=tip]`)
  await h.waitFor(() => h.blockName(FX, Y + 3, FZ) === 'pointed_dripstone', 3000)
  await h.tp(FX + 0.5, Y + 1, FZ + 0.5)
  await heal()
  levelEvents.length = 0
  damage.length = 0
  spawned.length = 0
  fallingAt.length = 0
  const before = bot.health
  h.cmd(`setblock ${FX} ${Y + 6} ${FZ} air`)
  await h.waitFor(() => spawned.filter(n => n === 'item').length >= 3, 5000)
  await h.ticks(20)
  h.result('fall_blocks_left', [5, 4, 3].map(dy => h.blockName(FX, Y + dy, FZ)).join(','))
  h.result('fall_entities', fallingAt.join(' '))
  h.result('fall_items', spawned.filter(n => n === 'item').length)
  h.result('fall_land_sounds', levelEvents.filter(e => e === 1045).length)
  h.result('fall_damage', (before - bot.health).toFixed(1))
  h.result('fall_damage_types', damage.join(','))
  h.result('fall_drops_picked', h.inv().pointed_dripstone || 0)
  await h.clear()
  h.cmd(`kill @e[type=item,x=${X},y=${Y},z=${Z},distance=..20]`)

  // A stalagmite whose floor goes breaks and drops itself.
  const SX = X - 3; const SZ = Z
  h.cmd(`setblock ${SX} ${Y + 3} ${SZ} stone`)
  h.cmd(`setblock ${SX} ${Y + 4} ${SZ} pointed_dripstone[vertical_direction=up,thickness=tip]`)
  await h.waitFor(() => h.blockName(SX, Y + 4, SZ) === 'pointed_dripstone', 3000)
  spawned.length = 0
  fallingAt.length = 0
  h.cmd(`setblock ${SX} ${Y + 3} ${SZ} air`)
  await h.waitFor(() => h.blockName(SX, Y + 4, SZ) === 'air', 3000)
  await h.ticks(10)
  h.result('stalagmite_unsupported', `${h.blockName(SX, Y + 4, SZ)}/${fallingAt.join(' ')}/${spawned.filter(n => n === 'item').length}`)
  h.cmd(`kill @e[type=item,x=${X},y=${Y},z=${Z},distance=..20]`)

  // The same drop onto stone, to tell fall damage apart from the stalagmite part.
  await heal()
  damage.length = 0
  const plainHigh = bot.health
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 6} ${Z - 4 + 0.5}`)
  await h.waitFor(() => damage.length > 0, 5000)
  await h.ticks(10)
  h.result('plain_fall_damage', (plainHigh - bot.health).toFixed(1))

  // Landing on a stalagmite tip.
  const LX = X; const LZ = Z + 4
  h.cmd(`setblock ${LX} ${Y + 1} ${LZ} pointed_dripstone[vertical_direction=up,thickness=tip]`)
  await h.waitFor(() => h.blockName(LX, Y + 1, LZ) === 'pointed_dripstone', 3000)
  await heal()
  damage.length = 0
  const high = bot.health
  h.cmd(`tp ${bot.username} ${LX + 0.5} ${Y + 6} ${LZ + 0.5}`)
  await h.waitFor(() => damage.length > 0, 5000)
  await h.ticks(10)
  h.result('stalagmite_damage', (high - bot.health).toFixed(1))
  h.result('stalagmite_damage_types', damage.join(','))
  await heal()
  h.cmd(`setblock ${LX} ${Y + 1} ${LZ} air`)

  // Growth: water two above the stalactite's top, sped up random ticks.
  const GX = X + 5; const GZ = Z + 5
  h.cmd(`fill ${GX - 1} ${Y + 7} ${GZ - 1} ${GX + 1} ${Y + 8} ${GZ + 1} stone`)
  h.cmd(`setblock ${GX} ${Y + 7} ${GZ} water`)
  h.cmd(`setblock ${GX} ${Y + 6} ${GZ} dripstone_block`)
  h.cmd(`setblock ${GX} ${Y + 5} ${GZ} pointed_dripstone[vertical_direction=down,thickness=tip]`)
  await h.tp(GX + 0.5, Y + 1, GZ - 2.5)
  await h.waitFor(() => h.blockName(GX, Y + 5, GZ) === 'pointed_dripstone', 3000)
  h.cmd('gamerule random_tick_speed 4096')
  await h.serverTicks(400)
  h.cmd('gamerule random_tick_speed 0')
  await h.ticks(10)
  let pieces = 0
  for (let dy = 1; dy <= 5; dy++) if (h.blockName(GX, Y + dy, GZ) === 'pointed_dripstone') pieces++
  h.result('grew', pieces > 1)
  h.cmd('gamerule random_tick_speed 3')
  h.cmd(`fill ${GX - 1} ${Y + 1} ${GZ - 1} ${GX + 1} ${Y + 8} ${GZ + 1} air`)
  await h.ticks(10)
})
