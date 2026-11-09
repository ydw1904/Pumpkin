// Light in chunk packets after block changes in the sky: a dark roofed platform /filled across
// several chunks, a torch under the roof, a hole in the roof, a block over open sky and a hole
// into a closed box. Mineflayer doesn't light chunks itself and chunks inside the view get no
// light updates (a real client lights them itself), so the bot leaves right away and comes back
// to have every chunk sent again, then reads the light and counts the light sections it got.
// Leaving at once also unloads the chunks while their light may still be updating. Vanilla
// sends light for each section next to blocks, counting neighbouring chunks, and nothing else.
const { make } = require('./survival-lib')
const h = make('Filler')
const { bot, Vec3 } = h
const X = 9000; const Y = 200; const Z = 8
const R = 24

const probes = {
  under_roof: [0, 2, 0],
  under_roof_edge: [R - 1, 2, 0],
  outside_roof: [R + 4, 2, 0],
  above_roof: [0, 8, 0],
  below_floor: [0, -3, 0],
  torch: [11, 1, 10],
  roof_hole: [-10, 3, -10],
  shadow: [40, 1, 0],
  box_inside: [33, -9, 33],
  box_corner: [31, -11, 31]
}

const sections = {}
const count = (mask) => Buffer.from(mask || []).reduce((n, byte) => {
  for (let b = byte; b; b &= b - 1) n++
  return n
}, 0)
bot._client.on('map_chunk', (p) => {
  sections[`${p.x},${p.z}`] = [p.skyLightMask, p.emptySkyLightMask, p.blockLightMask, p.emptyBlockLightMask].map(count)
})

const set = async (cmd, x, y, z, name) => {
  h.cmd(cmd)
  await h.waitFor(() => h.blockName(x, y, z) === name, 5000)
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd('gamerule advance_time false')
  h.cmd('time set 6000')

  await set(`fill ${X - R} ${Y} ${Z - R} ${X + R} ${Y} ${Z + R} stone`, X + R, Y, Z + R, 'stone')
  await set(`fill ${X - R} ${Y + 5} ${Z - R} ${X + R} ${Y + 5} ${Z + R} stone`, X + R, Y + 5, Z + R, 'stone')
  await set(`setblock ${X + 11} ${Y + 1} ${Z + 11} torch`, X + 11, Y + 1, Z + 11, 'torch')
  await set(`setblock ${X - 10} ${Y + 5} ${Z - 10} air`, X - 10, Y + 5, Z - 10, 'air')
  await set(`setblock ${X + 40} ${Y + 3} ${Z} stone`, X + 40, Y + 3, Z, 'stone')
  await set(`fill ${X + 30} ${Y - 12} ${Z + 30} ${X + 36} ${Y - 6} ${Z + 36} stone hollow`, X + 30, Y - 12, Z + 30, 'stone')
  await set(`setblock ${X + 33} ${Y - 6} ${Z + 33} air`, X + 33, Y - 6, Z + 33, 'air')

  h.cmd('gamemode creative Filler')
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  bot.creative.startFlying()
  await h.tp(X + 3000, Y + 1, Z)
  await h.tp(X + 0.5, Y + 1, Z + 0.5)
  await h.ticks(20)

  for (const [name, [dx, dy, dz]] of Object.entries(probes)) {
    const pos = new Vec3(X + dx, Y + dy, Z + dz)
    h.result(name, `sky=${bot.world.getSkyLight(pos)} block=${bot.world.getBlockLight(pos)}`)
  }
  // A coarse grid over the platform and the chunks around it, per height.
  for (const dy of [-8, -3, 2, 6, 12, 24]) {
    let sum = 0; let zeros = 0
    for (let dx = -48; dx <= 48; dx += 8) {
      for (let dz = -48; dz <= 48; dz += 8) {
        const sky = bot.world.getSkyLight(new Vec3(X + dx, Y + dy, Z + dz))
        sum += sky
        zeros += sky === 0
      }
    }
    h.result(`grid_dy${dy}`, `sky_sum=${sum} zeros=${zeros}`)
  }
  // Light sections (sky, empty sky, block, empty block) of the 7x7 chunks around the platform.
  const totals = [0, 0, 0, 0]
  const cx = Math.floor(X / 16); const cz = Math.floor(Z / 16)
  for (let dx = -3; dx <= 3; dx++) {
    for (let dz = -3; dz <= 3; dz++) {
      (sections[`${cx + dx},${cz + dz}`] || []).forEach((n, i) => { totals[i] += n })
    }
  }
  h.result('light_sections', totals.join(','))
})
