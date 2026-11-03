// Nether portals: which frames flint and steel lights, travel delay, coordinate scaling, the
// PortalForcer's portal creation (in a prepared cave, and the obsidian platform when there is no
// room), linking to existing portals (16 blocks in the nether, 128 in the overworld), and mobs
// and items riding along without bouncing back while they stand in the exit portal.
// pumpkin-settings: allow_nether=true
const { make } = require('./survival-lib')
const h = make('Porter')
const { bot, Vec3 } = h
const X = 3000; const Y = 200; const Z = 0
const NX = 375 // X / 8

const fill = (dim, a, b, block) => h.cmd(`execute in minecraft:${dim} run fill ${a.join(' ')} ${b.join(' ')} ${block}`)
const dimName = () => String(bot.game.dimension).replace('minecraft:', '')

// An obsidian frame with a `w` x `ht` opening whose bottom row is at y0, along X or Z.
function frame (x0, y0, z0, w, ht, axis = 'x', dim = 'overworld') {
  const end = axis === 'x' ? [x0 + w + 1, y0 + ht + 1, z0] : [x0, y0 + ht + 1, z0 + w + 1]
  fill(dim, [x0, y0, z0], end, 'obsidian')
  const ia = axis === 'x' ? [x0 + 1, y0 + 1, z0] : [x0, y0 + 1, z0 + 1]
  const ib = axis === 'x' ? [x0 + w, y0 + ht, z0] : [x0, y0 + ht, z0 + w]
  fill(dim, ia, ib, 'air')
}

function countPortal (x0, y0, z0, w, ht, axis = 'x') {
  let n = 0
  for (let i = 1; i <= w; i++) {
    for (let j = 1; j <= ht; j++) {
      const p = axis === 'x' ? [x0 + i, y0 + j, z0] : [x0, y0 + j, z0 + i]
      if (h.blockName(...p) === 'nether_portal') n++
    }
  }
  return n
}

async function light (stand, target) {
  await h.tp(stand[0], stand[1], stand[2])
  await h.ticks(5)
  if (!bot.heldItem || bot.heldItem.name !== 'flint_and_steel') {
    await h.give('flint_and_steel')
    await bot.equip(bot.inventory.items().find(i => i.name === 'flint_and_steel'), 'hand')
  }
  const block = h.block(...target)
  await bot.lookAt(new Vec3(target[0] + 0.5, target[1] + 1, target[2] + 0.5), true)
  await bot.activateBlock(block, new Vec3(0, 1, 0)).catch(e => h.result('light_error', e.message))
  await h.ticks(10)
}

// Walks into the portal at (x, y, z) and waits for the dimension to change; returns the ticks it took.
async function enter (x, y, z) {
  const from = dimName()
  h.cmd(`execute in minecraft:${from} run tp ${bot.username} ${x} ${y} ${z}`)
  const start = bot.time.age
  const ok = await h.waitFor(() => dimName() !== from, 30000)
  const ticks = ok ? Number(bot.time.age - start) : -1
  await h.waitFor(() => bot.entity && bot.entity.position && h.chunksLoaded(), 60000)
  await h.ticks(20)
  return ticks
}

function portalsNear (r = 16) {
  return bot.findBlocks({ matching: (b) => b && b.name === 'nether_portal', maxDistance: r, count: 200 })
}
function box (list) {
  if (!list.length) return 'none'
  const xs = list.map(p => p.x); const ys = list.map(p => p.y); const zs = list.map(p => p.z)
  const axis = h.block(list[0].x, list[0].y, list[0].z).getProperties().axis
  return `${Math.min(...xs)},${Math.min(...ys)},${Math.min(...zs)}..${Math.max(...xs)},${Math.max(...ys)},${Math.max(...zs)} axis=${axis} n=${list.length}`
}
const pos = () => { const p = bot.entity.position; return `${p.x.toFixed(2)},${p.y.toFixed(2)},${p.z.toFixed(2)}` }
const near = (name, r = 8) => Object.values(bot.entities).filter(e => e.name === name && e.position.distanceTo(bot.entity.position) < r).length

h.start(async () => {
  // Nether ground for the first trip: solid netherrack around the destination with one cave.
  h.cmd('execute in minecraft:the_nether run forceload add 352 -32 400 32')
  h.cmd('execute in minecraft:the_nether run forceload add 432 -32 470 32')
  h.cmd('execute in minecraft:the_nether run forceload add 512 -32 560 32')
  // Forced chunks load in the background; a lamp at spawn lights up once they all have.
  const sx = Math.floor(bot.entity.position.x); const sy = Math.floor(bot.entity.position.y) + 3; const sz = Math.floor(bot.entity.position.z)
  for (let i = 0; i < 120 && h.blockName(sx, sy, sz) !== 'glowstone'; i++) {
    h.cmd(`execute in minecraft:the_nether if loaded 352 64 -32 if loaded 400 64 32 if loaded 432 64 -32 if loaded 470 64 32 if loaded 512 64 -32 if loaded 560 64 32 run execute in minecraft:overworld run setblock ${sx} ${sy} ${sz} glowstone`)
    await h.sleep(1000)
  }
  h.cmd(`setblock ${sx} ${sy} ${sz} air`)
  for (let y = 0; y < 128; y += 19) fill('the_nether', [355, y, -20], [395, Math.min(y + 18, 127), 20], 'netherrack')
  fill('the_nether', [370, 64, -4], [380, 70, 4], 'air')
  for (let y = 0; y < 128; y += 19) fill('the_nether', [432, y, -20], [470, Math.min(y + 18, 127), 20], 'netherrack')
  // An existing portal 13 blocks from where the third trip lands.
  fill('the_nether', [530, 60, -4], [548, 72, 4], 'air')
  fill('the_nether', [530, 59, -4], [548, 59, 4], 'netherrack')
  frame(537, 60, 0, 2, 3, 'x', 'the_nether')
  fill('the_nether', [538, 61, 0], [539, 63, 0], 'nether_portal[axis=x]')
  await h.ticks(20)

  await h.arena(X, Y, Z, 12)
  await h.clear()
  const y0 = Y + 1
  frame(X - 10, y0, Z - 10, 2, 3) // A: smallest
  frame(X - 4, y0, Z - 10, 1, 3) // B: too narrow
  frame(X + 2, y0, Z - 10, 2, 3) // G: with a hole in the side
  h.cmd(`setblock ${X + 2} ${y0 + 2} ${Z - 10} air`)
  frame(X - 12, y0, Z - 6, 22, 3) // D: too wide
  frame(X - 10, y0, Z - 2, 2, 3) // E: no corners
  for (const [cx, cy] of [[X - 10, y0], [X - 7, y0], [X - 10, y0 + 4], [X - 7, y0 + 4]]) h.cmd(`setblock ${cx} ${cy} ${Z - 2} air`)
  frame(X + 6, y0, Z - 3, 3, 4, 'z') // F: along Z
  frame(X - 11, y0, Z + 6, 21, 21) // C: largest
  await h.ticks(20)

  await light([X - 8.5, Y + 1, Z - 8.5], [X - 9, y0, Z - 10])
  h.result('light_2x3', countPortal(X - 10, y0, Z - 10, 2, 3))
  await light([X - 2.5, Y + 1, Z - 8.5], [X - 3, y0, Z - 10])
  h.result('light_1x3', countPortal(X - 4, y0, Z - 10, 1, 3) + ' ' + h.blockName(X - 3, y0 + 1, Z - 10))
  await light([X + 3.5, Y + 1, Z - 8.5], [X + 3, y0, Z - 10])
  h.result('light_with_hole', countPortal(X + 2, y0, Z - 10, 2, 3))
  await light([X + 0.5, Y + 1, Z - 4.5], [X, y0, Z - 6])
  h.result('light_22x3', countPortal(X - 12, y0, Z - 6, 22, 3))
  await light([X - 8.5, Y + 1, Z - 0.5], [X - 9, y0, Z - 2])
  h.result('light_no_corners', countPortal(X - 10, y0, Z - 2, 2, 3))
  await light([X + 4.5, Y + 1, Z - 1.5], [X + 6, y0, Z - 2])
  h.result('light_along_z', countPortal(X + 6, y0, Z - 3, 3, 4, 'z'))
  await light([X + 0.5, Y + 1, Z + 4.5], [X, y0, Z + 6])
  h.result('light_21x21', countPortal(X - 11, y0, Z + 6, 21, 21))
  // Breaking one frame block takes the whole portal down.
  h.cmd(`setblock ${X + 6} ${y0 + 2} ${Z + 1} air`)
  await h.ticks(5)
  h.result('broken_frame_portal', countPortal(X + 6, y0, Z - 3, 3, 4, 'z'))

  // A pig and an item go through first, then the player.
  h.cmd(`summon pig ${X - 8.5} ${y0 + 1} ${Z - 9.5} {NoAI:1b}`)
  h.cmd(`summon item ${X - 8} ${y0 + 1} ${Z - 9.5} {Item:{id:"minecraft:diamond",count:1},PickupDelay:32767}`)
  await h.ticks(10)
  h.result('pig_left_overworld', near('pig', 20) === 0)
  h.result('item_left_overworld', near('item', 20) === 0)
  h.result('survival_delay', await enter(X - 8.5, y0 + 1, Z - 9.5))
  h.result('trip1_dim', dimName())
  h.result('trip1_pos', pos())
  h.result('trip1_portal', box(portalsNear()))
  h.result('trip1_pig_item', `${near('pig')},${near('item')}`)
  await h.ticks(400)
  h.result('trip1_still_nether', dimName())
  h.result('trip1_pig_item_after_400', `${near('pig')},${near('item')}`)

  // Back through the same portal: it links to portal A.
  const exit = portalsNear()
  const ex = Math.min(...exit.map(p => p.x)); const ey = Math.min(...exit.map(p => p.y)); const ez = Math.min(...exit.map(p => p.z))
  h.cmd(`execute in minecraft:the_nether run tp ${bot.username} ${ex + 0.5} ${ey} ${ez + 2.5}`)
  await h.ticks(30)
  h.result('return_delay', await enter(ex + 0.5, ey, ez + 0.5))
  h.result('return_dim', dimName())
  h.result('return_pos', pos())

  // Second trip: no room near the destination, so the forcer builds a platform.
  h.cmd(`execute in minecraft:overworld run tp ${bot.username} ${X + 600.5} ${Y + 1} ${Z + 0.5}`)
  await h.waitFor(() => dimName() === 'overworld', 20000)
  await h.arena(X + 600, Y, Z, 4)
  frame(X + 599, y0, Z, 2, 3)
  h.cmd(`fill ${X + 600} ${y0 + 1} ${Z} ${X + 601} ${y0 + 3} ${Z} nether_portal[axis=x]`)
  await h.ticks(10)
  await enter(X + 600.5, y0 + 1, Z + 0.5)
  h.result('trip2_dim', dimName())
  h.result('trip2_pos', pos())
  h.result('trip2_portal', box(portalsNear()))
  const plat = []
  for (let dx = -2; dx <= 3; dx++) for (let dz = -2; dz <= 2; dz++) plat.push(h.blockName(Math.floor(bot.entity.position.x) + dx, Math.floor(bot.entity.position.y) - 1, Math.floor(bot.entity.position.z) + dz)[0])
  h.result('trip2_floor', plat.join(''))

  // Third trip: an existing portal 13 blocks away in the nether is used, and the way back finds
  // the overworld portal 104 blocks from the scaled position.
  h.cmd(`execute in minecraft:overworld run tp ${bot.username} ${X + 1200.5} ${Y + 1} ${Z + 0.5}`)
  await h.waitFor(() => dimName() === 'overworld', 20000)
  await h.arena(X + 1200, Y, Z, 4)
  frame(X + 1199, y0, Z, 2, 3)
  h.cmd(`fill ${X + 1200} ${y0 + 1} ${Z} ${X + 1201} ${y0 + 3} ${Z} nether_portal[axis=x]`)
  await h.ticks(10)
  await enter(X + 1200.5, y0 + 1, Z + 0.5)
  h.result('trip3_dim', dimName())
  h.result('trip3_pos', pos())
  h.result('trip3_portal', box(portalsNear()))
  h.cmd(`execute in minecraft:the_nether run tp ${bot.username} 538.5 61 2.5`)
  await h.ticks(30)
  await enter(538.5, 61, 0.5)
  h.result('trip3_return_dim', dimName())
  h.result('trip3_return_pos', pos())
  h.cmd('execute in minecraft:the_nether run forceload remove all')
})
