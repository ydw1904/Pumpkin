// Items in the cursor and the 2x2 grid are dropped when the player leaves. They are thrown
// forward like any drop and keep that motion while their chunk is unloaded, so after rejoining
// they land out of reach instead of at the player's feet.
const mineflayer = require('mineflayer')
const { make } = require('./survival-lib')
const h = make('Leaver')
const { bot } = h
const X = 2400; const Y = 200; const Z = 0

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  await h.fill({ 9: 'oak_log 5', 10: 'dirt 3' })
  await h.click(9, 0, 0); await h.click(1, 1, 0); await h.click(2, 1, 0); await h.click(10, 0, 0)
  await h.ticks(5)
  bot.quit()
  await h.sleep(4000)
  const b2 = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username: 'Leaver', version: '26.3', auth: 'offline' })
  let collected = 0
  b2._client.on('collect', () => collected++)
  await new Promise((resolve) => b2.once('spawn', resolve))
  await h.sleep(10000)
  const inv = {}
  for (const it of b2.inventory.items()) inv[it.name] = (inv[it.name] || 0) + it.count
  h.result('rejoin_inventory', Object.keys(inv).sort().map(k => `${k}x${inv[k]}`).join(',') || 'empty')
  h.result('rejoin_collected', collected)
  b2.quit()
})
