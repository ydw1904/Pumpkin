// End portals: filling a frame ring with eyes of ender opens the portal only when every frame
// faces inward, eyes aren't used on a filled frame, and players and items arrive on the
// obsidian platform in the End.
// pumpkin-settings: allow_end=true
const { make } = require('./survival-lib')
const h = make('Ender')
const { bot, Vec3 } = h
const X = 7000; const Y = 200; const Z = 0
const dimName = () => String(bot.game.dimension).replace('minecraft:', '')

// The 12 frames around a 3x3 opening at (x..x+2, z..z+2), facing inward.
function ring (x, z) {
  const frames = []
  for (let i = 0; i < 3; i++) {
    frames.push([x + i, z - 1, 'south'], [x + i, z + 3, 'north'], [x - 1, z + i, 'east'], [x + 3, z + i, 'west'])
  }
  return frames
}
const portalCount = (x, z) => { let n = 0; for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) if (h.blockName(x + i, Y + 1, z + j) === 'end_portal') n++; return n }

async function fillEyes (frames) {
  // Stand outside the ring, behind the frame.
  const away = { south: [0, -1.5], north: [0, 1.5], east: [-1.5, 0], west: [1.5, 0] }
  for (const [fx, fz, facing] of frames) {
    const [ax, az] = away[facing]
    await h.tp(fx + 0.5 + ax, Y + 1, fz + 0.5 + az)
    await bot.equip(bot.inventory.items().find(i => i.name === 'ender_eye'), 'hand')
    await bot.lookAt(new Vec3(fx + 0.5, Y + 1.8, fz + 0.5), true)
    await bot.activateBlock(h.block(fx, Y + 1, fz), new Vec3(0, 1, 0)).catch(e => h.result('eye_error', e.message))
    await h.ticks(4)
  }
}

h.start(async () => {
  await h.arena(X + 6, Y, Z + 1, 12)
  await h.clear()
  const good = ring(X, Z)
  const bad = ring(X + 8, Z)
  for (const [fx, fz, facing] of good) h.cmd(`setblock ${fx} ${Y + 1} ${fz} end_portal_frame[facing=${facing}]`)
  bad.forEach(([fx, fz, facing], i) => h.cmd(`setblock ${fx} ${Y + 1} ${fz} end_portal_frame[facing=${i === 4 ? 'north' : facing}]`))
  await h.ticks(10)
  await h.give('ender_eye', 30)
  await fillEyes(good.slice(0, 11))
  h.result('eleven_eyes_portal', portalCount(X, Z))
  await fillEyes(good.slice(11))
  h.result('twelve_eyes_portal', portalCount(X, Z))
  h.result('eyes_used', 30 - (h.inv().ender_eye || 0))
  await fillEyes(good.slice(0, 1))
  h.result('eye_on_filled_frame_used', 30 - (h.inv().ender_eye || 0))
  await fillEyes(bad)
  h.result('wrong_facing_portal', portalCount(X + 8, Z))

  h.cmd(`summon item ${X + 1.5} ${Y + 1.2} ${Z + 1.5} {Item:{id:"minecraft:diamond",count:1},PickupDelay:32767}`)
  await h.ticks(20)
  h.result('item_left', Object.values(bot.entities).filter(e => e.name === 'item').length === 0)
  h.cmd(`tp ${bot.username} ${X + 1.5} ${Y + 1.2} ${Z + 1.5}`)
  await h.waitFor(() => dimName() === 'the_end', 20000)
  await h.waitFor(h.chunksLoaded, 60000)
  await h.ticks(40)
  h.result('end_dim', dimName())
  const p = bot.entity.position
  h.result('end_pos', `${p.x.toFixed(1)},${p.y.toFixed(1)},${p.z.toFixed(1)}`)
  const floor = []
  for (let dx = -2; dx <= 2; dx++) for (let dz = -2; dz <= 2; dz++) floor.push(h.blockName(100 + dx, 48, dz) === 'obsidian' ? 'o' : '.')
  h.result('end_platform', floor.join(''))
  let open = true
  for (let dx = -2; dx <= 2; dx++) for (let dz = -2; dz <= 2; dz++) for (let dy = 49; dy <= 51; dy++) if (h.blockName(100 + dx, dy, dz) !== 'air') open = false
  h.result('end_platform_cleared', open)
  h.result('end_item_arrived', Object.values(bot.entities).filter(e => e.name === 'item' && e.position.distanceTo(new Vec3(100.5, 50, 0.5)) < 4).length)
  h.cmd(`kill ${bot.username}`)
  await h.respawnAfterDeath()
  h.result('respawn_dim', dimName())
})
