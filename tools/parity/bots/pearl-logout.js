// A player's ender pearls in flight leave with them when they log out and come back when they
// log in again (vanilla `ServerPlayer.ender_pearls`). A second bot watches the pearl.
const mineflayer = require('mineflayer')
const { make } = require('./survival-lib')
const h = make('Pearler')
const { bot, Vec3 } = h
const X = 5600; const Y = 200; const Z = 0

const ownerTag = () => `Owner:[I;${bot.player.uuid.replace(/-/g, '').match(/.{8}/g).map((s) => parseInt(s, 16) | 0).join(',')}]`
const join = (username) => new Promise((resolve) => {
  const b = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username, version: '26.3', auth: 'offline' })
  b.on('error', (e) => console.log('ERROR', e.message))
  b.once('spawn', () => resolve(b))
})
const pearls = (watcher) => Object.values(watcher.entities).filter(e => e.name === 'ender_pearl' && e.position.distanceTo(new Vec3(X + 0.5, Y + 4, Z + 8.5)) < 4).length

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  const watcher = await join('Watcher')
  h.cmd(`tp Watcher ${X + 0.5} ${Y + 1} ${Z + 3.5}`)
  h.cmd('gamemode creative Watcher')
  await h.waitFor(() => watcher.entity && watcher.entity.position.distanceTo(new Vec3(X + 0.5, Y + 1, Z + 3.5)) < 1.5, 20000)
  await watcher.waitForTicks(20)
  h.cmd(`summon ender_pearl ${X + 0.5} ${Y + 4} ${Z + 8.5} {Motion:[0.0,0.0,0.0],NoGravity:1b,${ownerTag()}}`)
  await watcher.waitForTicks(20)
  h.result('before_logout', pearls(watcher))

  bot.quit()
  await h.waitFor(() => !Object.values(watcher.entities).some(e => e.username === 'Pearler'), 10000)
  await watcher.waitForTicks(40)
  h.result('after_logout', pearls(watcher))

  const back = await join('Pearler')
  await h.waitFor(() => back.entity && back.entity.position, 10000)
  await watcher.waitForTicks(40)
  h.result('after_rejoin', pearls(watcher))
  h.cmd(`kill @e[type=ender_pearl,x=${X},y=${Y},z=${Z},distance=..20]`)
  await watcher.waitForTicks(10)
  back.quit()
  watcher.quit()
})
