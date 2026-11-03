// Explorer maps: a cartographer's map trade makes a map centered on the map grid around the
// structure, with the target marker relative to the center and the water biomes outlined.
// The trade's map is bought and held in the Nether, where neither server draws terrain on it,
// to read the map packet.
// pumpkin-settings: allow_nether=true
const { make } = require('./survival-lib')
const h = make('Explorer')
const { bot } = h
const X = 0; const Y = 200; const Z = 0

const maps = {}
bot._client.on('map', (p) => {
  const m = maps[p.itemDamage] || (maps[p.itemDamage] = { colors: Buffer.alloc(128 * 128) })
  m.scale = p.scale
  m.tracking = p.trackingPosition
  m.icons = (p.icons || []).map((i) => `${i.type}@${i.x},${i.z}r${i.direction}`).sort().join(' ')
  if (p.columns > 0 && p.data) {
    for (let x = 0; x < p.columns; x++) for (let y = 0; y < p.rows; y++) m.colors[p.x + x + (p.y + y) * 128] = p.data[x + y * p.columns]
  }
})

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  await h.clear()
  h.cmd(`summon villager ${X + 2.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,VillagerData:{profession:"minecraft:cartographer",level:5,type:"minecraft:plains"},Xp:250}`)
  await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'villager'), 5000)
  await h.ticks(10)
  await h.give('emerald', 64)
  await h.give('compass', 4)
  const villager = await bot.openVillager(Object.values(bot.entities).find(e => e.name === 'villager'))
  await h.waitFor(() => villager.trades && villager.trades.length > 0, 30000)
  const mapTrades = villager.trades.map((t, i) => [t, i]).filter(([t]) => t.outputItem.name.endsWith('_map'))
  h.result('map_trades', mapTrades.map(([t]) => t.outputItem.name).sort().join(' ') || 'none')
  for (const [t, i] of mapTrades) {
    await bot.trade(villager, i, 1).catch((e) => h.result(`trade_error.${t.outputItem.name}`, e.message.slice(0, 80)))
  }
  bot.closeWindow(villager)
  await h.ticks(10)
  h.cmd('execute in minecraft:the_nether run tp Explorer 0.5 120 0.5')
  await h.waitFor(() => (bot.game.dimension || '').includes('nether'), 20000)
  await h.ticks(40)
  // Hold each bought map in the main hand so the server sends its data.
  const bought = bot.inventory.items().filter((it) => it.name.endsWith('_map'))
  for (const it of bought) {
    await bot.equip(it, 'hand')
    await h.ticks(30)
  }
  // The structures differ per server only if world generation does, so ids are left out.
  const shown = Object.values(maps).map((m) => `scale=${m.scale} tracking=${m.tracking} icons=[${m.icons}] painted=${m.colors.filter((c) => c !== 0).length}`).sort()
  h.result('maps', shown.join(' | ') || 'none')
})
