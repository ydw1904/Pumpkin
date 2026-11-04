// Light update packets for runtime light changes: vanilla sends the changed sections only to
// players that hold the chunk at the border of their view, and the client lights the others
// itself. Places glowstone and a sky-blocking stone in a chunk inside the view and in one at its
// border and reports the light packets and the client's light values.
const { make } = require('./survival-lib')
const h = make('Lighter')
const { bot, Vec3 } = h
const X = 8000; const Y = 150; const Z = 8

const packets = []
bot._client.on('packet', (data, meta) => {
  if (meta.name === 'update_light') packets.push(data)
})
const describe = (cx, cz) => {
  const mine = packets.filter(p => p.chunkX === cx && p.chunkZ === cz)
  const sky = mine.reduce((n, p) => n + p.skyLight.length, 0)
  const block = mine.reduce((n, p) => n + p.blockLight.length, 0)
  return `packets=${mine.length} sky_arrays=${sky} block_arrays=${block}`
}
const loaded = (cx, cz) => !!bot.world.getColumn(cx, cz)

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd('gamerule advance_time false')
  h.cmd('time set 6000')
  // Let the whole view arrive before looking for its border.
  let count = -1
  for (let i = 0; i < 60; i++) {
    const now = Object.keys(bot.world.async.columns).length
    if (now === count) break
    count = now
    await h.sleep(1000)
  }
  const cx = Math.floor(bot.entity.position.x / 16)
  const cz = Math.floor(bot.entity.position.z / 16)
  let border = cx
  while (loaded(border + 1, cz)) border++
  const inner = cx + 2
  h.result('border_offset', border - cx)

  const place = async (chunkX, dy, block) => {
    h.cmd(`setblock ${chunkX * 16 + 8} ${Y + dy} ${cz * 16 + 8} ${block}`)
    await h.waitFor(() => h.blockName(chunkX * 16 + 8, Y + dy, cz * 16 + 8) === block, 5000)
    await h.ticks(20)
  }
  const light = (chunkX, dy) => {
    const pos = new Vec3(chunkX * 16 + 9, Y + dy, cz * 16 + 8)
    return `block=${bot.world.getBlockLight(pos)} sky=${bot.world.getSkyLight(pos)}`
  }

  for (const [name, chunkX] of [['inner', inner], ['border', border]]) {
    packets.length = 0
    await place(chunkX, 3, 'glowstone')
    h.result(`${name}_glowstone`, describe(chunkX, cz))
    h.result(`${name}_glowstone_light`, light(chunkX, 3))
    packets.length = 0
    await place(chunkX, 3, 'air')
    h.result(`${name}_glowstone_removed`, describe(chunkX, cz))
    h.result(`${name}_glowstone_removed_light`, light(chunkX, 3))
    packets.length = 0
    await place(chunkX, 20, 'stone')
    h.result(`${name}_roof`, describe(chunkX, cz))
    h.result(`${name}_roof_light`, light(chunkX, 19))
    h.result(`${name}_neighbour_packets`, describe(chunkX - 1, cz))
  }
})
