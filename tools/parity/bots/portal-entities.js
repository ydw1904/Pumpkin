// Non-player entities in a nether portal: an item, an arrow, a minecart with a pig, a boat, a
// falling block and primed TNT go through vanilla `handlePortal` and arrive at the exit portal.
// The exit portal is turned 90 degrees, so the arrow's motion turns with it (`ROTATE_DELTA`).
// The falling block floats: one landing in the exit portal drops an item that travels back.
// An item teleported onto another merges with it at once.
// pumpkin-settings: allow_nether=true
const { make } = require('./survival-lib')
const h = make('Portaler')
const { bot, Vec3 } = h
const X = 4400; const Y = 200; const Z = 0
// The nether side, near X / 8.
const NX = 550

const fill = (dim, a, b, block) => h.cmd(`execute in minecraft:${dim} run fill ${a.join(' ')} ${b.join(' ')} ${block}`)
const dimName = () => String(bot.game.dimension).replace('minecraft:', '')
const near = (name, x, y, z, r) => Object.values(bot.entities).filter(e => e.name === name && e.position.distanceTo(new Vec3(x, y, z)) < r)
const itemName = (e) => { try { const it = e.getDroppedItem(); return it ? it.name : '?' } catch (err) { return '?' } }

const riders = {}
bot._client.on('set_passengers', ({ entityId, passengers }) => { riders[entityId] = passengers })
const riderNames = (e) => (riders[e.id] || []).map(id => bot.entities[id] ? bot.entities[id].name : `id${id}`).join('+') || 'none'

h.start(async () => {
  // The nether side: a closed netherrack room with a portal along Z in the middle.
  h.cmd(`execute in minecraft:the_nether run forceload add ${NX - 16} -16 ${NX + 16} 16`)
  const sx = Math.floor(bot.entity.position.x); const sy = Math.floor(bot.entity.position.y) + 3; const sz = Math.floor(bot.entity.position.z)
  for (let i = 0; i < 120 && h.blockName(sx, sy, sz) !== 'glowstone'; i++) {
    h.cmd(`execute in minecraft:the_nether if loaded ${NX - 16} 64 -16 if loaded ${NX + 16} 64 16 run execute in minecraft:overworld run setblock ${sx} ${sy} ${sz} glowstone`)
    await h.sleep(1000)
  }
  h.cmd(`setblock ${sx} ${sy} ${sz} air`)
  fill('the_nether', [NX - 12, 58, -8], [NX + 12, 72, 8], 'netherrack')
  fill('the_nether', [NX - 10, 60, -6], [NX + 10, 70, 6], 'air')
  fill('the_nether', [NX, 60, -2], [NX, 64, 1], 'obsidian')
  fill('the_nether', [NX, 61, -1], [NX, 63, 0], 'nether_portal[axis=z]')

  // The overworld side: a portal along X.
  await h.arena(X, Y, Z, 8)
  // Vanilla `ItemEntity.teleport` merges with neighbours at once, not on the next merge tick.
  const emerald = '{Item:{id:"minecraft:emerald",count:1},PickupDelay:200,Tags:["tp_merge"]}'
  h.cmd(`summon item ${X - 4.5} ${Y + 1} ${Z + 5.5} ${emerald}`)
  h.cmd(`summon item ${X - 1.5} ${Y + 1} ${Z + 5.5} ${emerald}`)
  await h.ticks(30)
  h.cmd(`tp @e[type=item,tag=tp_merge,x=${X - 4.5},y=${Y + 1},z=${Z + 5.5},distance=..1] ${X - 1.5} ${Y + 1} ${Z + 5.5}`)
  await h.waitFor(() => near('item', X - 4.5, Y + 1, Z + 5.5, 1).length === 0, 5000)
  await h.ticks(2)
  h.result('tp_merge', `${near('item', X - 1.5, Y + 1, Z + 5.5, 2).length}`)
  h.cmd('kill @e[type=item,tag=tp_merge]')
  await h.ticks(5)

  fill('overworld', [X - 1, Y + 1, Z], [X + 2, Y + 5, Z], 'obsidian')
  fill('overworld', [X, Y + 2, Z], [X + 1, Y + 4, Z], 'nether_portal[axis=x]')
  await h.ticks(20)
  h.result('portals', `${h.blockName(X, Y + 2, Z)},${h.blockName(NX, 61, 0)}`)

  h.cmd(`summon item ${X + 0.5} ${Y + 2} ${Z + 0.5} {Item:{id:"minecraft:diamond",count:1},PickupDelay:32767}`)
  h.cmd(`summon arrow ${X + 0.5} ${Y + 3} ${Z - 2.5} {Motion:[0.0,0.0,0.5],NoGravity:1b,pickup:0b}`)
  h.cmd(`summon minecart ${X + 1.5} ${Y + 2} ${Z + 0.5} {Passengers:[{id:"minecraft:pig",NoAI:1b}]}`)
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 2.5} ${Z + 0.5}`)
  h.cmd(`summon falling_block ${X + 1.5} ${Y + 4} ${Z + 0.5} {BlockState:{Name:"minecraft:sand"},Time:1,NoGravity:1b}`)
  h.cmd(`summon tnt ${X + 1.5} ${Y + 3} ${Z + 0.5} {fuse:1000}`)
  await h.ticks(40)
  const types = ['item', 'arrow', 'minecart', 'pig', 'oak_boat', 'falling_block', 'tnt']
  h.result('left_overworld', types.map(n => `${n}=${near(n, X + 1, Y + 3, Z, 10).length}`).join(','))
  h.result('overworld_items', near('item', X + 1, Y + 3, Z, 10).map(itemName).sort().join(',') || 'none')

  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 2} ${Z + 0.5}`)
  await h.waitFor(() => dimName() === 'the_nether', 30000)
  await h.waitFor(() => bot.entity && bot.entity.position && h.chunksLoaded(), 60000)
  await h.ticks(40)
  h.result('nether_dim', dimName())
  h.result('nether_arrivals', ['minecart', 'pig', 'oak_boat', 'tnt', 'falling_block'].map(n => `${n}=${near(n, NX, 61, 0, 8).length}`).join(','))
  const sand = []
  for (let x = NX - 3; x <= NX + 3; x++) for (let y = 60; y <= 66; y++) for (let z = -4; z <= 4; z++) if (h.blockName(x, y, z) === 'sand') sand.push(`${x - NX},${y},${z}`)
  h.result('nether_sand', sand.join(';') || 'none')
  h.result('nether_items', near('item', NX, 61, 0, 8).map(itemName).sort().join(',') || 'none')
  const cart = near('minecart', NX, 61, 0, 8)[0]
  h.result('minecart_riders', cart ? riderNames(cart) : 'no_minecart')
  const arrow = near('arrow', NX, 65, 0, 16)[0]
  h.result('arrow_stuck', arrow ? `${Math.floor(arrow.position.x) - NX},${Math.floor(arrow.position.z)}` : 'none')
  h.cmd('execute in minecraft:the_nether run kill @e[type=tnt]')
  h.cmd(`execute in minecraft:the_nether run forceload remove ${NX - 16} -16 ${NX + 16} 16`)
})
