// How long entities keep ticking after the player leaves their area: an item is summoned in the
// same console batch as a /tp away or a kick, the world is frozen with /tick freeze before the
// player comes back, and the item's Age tells how many ticks it ran. Vanilla moves a player's
// simulation ticket on ChunkMap.move (a client move or the teleport confirmation), and removes it
// at once on a disconnect. Exact ages depend on when the confirmation lands, so they're TIMING
// lines; the RESULT lines only bucket them.
const mineflayer = require('mineflayer')
const fs = require('fs')
const { make } = require('./survival-lib')
const NAME = 'Leaver2'
const h = make(NAME)
const { bot } = h
const Y = 200
const AWAY = 2000

// Several console lines in one write run in the same server tick on both servers.
const cmds = (...lines) => fs.appendFileSync(process.env.CMDS, lines.join('\n') + '\n')
const itemNbt = '{Item:{id:"minecraft:stone",count:1},NoGravity:1b,PickupDelay:32767s,Age:0s}'
const itemNear = (b, x, z) => Object.values(b.entities).find(e => e.name === 'item' && Math.abs(e.position.x - x) < 0.5 && Math.abs(e.position.z - z) < 0.5)

async function queryAge (b, x, z) {
  let age = null
  const listener = (msg) => { const m = /entity data: (-?\d+)s/.exec(msg); if (m) age = Number(m[1]) }
  b.on('messagestr', listener)
  b.chat(`/data get entity @e[type=item,x=${x},y=${Y},z=${z},distance=..1,limit=1] Age`)
  await h.waitFor(() => age !== null, 5000)
  b.removeListener('messagestr', listener)
  return age
}

async function goTo (x, z) {
  h.cmd(`tp ${NAME} ${x} ${Y} ${z}`)
  await h.waitFor(() => Math.abs(bot.entity.position.x - x) < 1 && Math.abs(bot.entity.position.z - z) < 1, 10000)
  await h.waitFor(h.chunksLoaded, 60000)
  await h.sleep(1500)
}

async function withoutConfirm (ticks) {
  const write = bot._client.write.bind(bot._client)
  const confirms = []
  bot._client.write = (name, params) => {
    if (name === 'teleport_confirm') confirms.push(params)
    else if (!['position', 'position_look', 'look', 'flying'].includes(name)) write(name, params)
  }
  const start = bot.time.age
  await h.serverTicks(ticks)
  console.log(`TIMING confirmation held for ${bot.time.age - start} ticks of world age`)
  bot._client.write = write
  for (const params of confirms) write('teleport_confirm', params)
}

// Summons the item next to the player at (x, z), leaves with `leave`, and comes back frozen.
async function scene (name, x, z, leave) {
  await goTo(x + 0.5, z + 0.5)
  const ix = x + 3.5; const iz = z + 0.5
  cmds(`summon item ${ix} ${Y} ${iz} ${itemNbt}`, `tp ${NAME} ${x + AWAY + 0.5} ${Y} ${z + 0.5}`)
  await leave()
  await h.waitFor(() => Math.abs(bot.entity.position.x - (x + AWAY + 0.5)) < 1, 10000)
  await h.sleep(3000)
  h.cmd('tick freeze')
  await h.sleep(500)
  await goTo(x + 0.5, z + 0.5)
  await h.waitFor(() => itemNear(bot, ix, iz), 10000)
  const age = await queryAge(bot, ix, iz)
  h.cmd('tick unfreeze')
  console.log(`TIMING ${name}: item ticked ${age} times after the player left`)
  return age
}

h.start(async () => {
  h.cmd(`gamemode creative ${NAME}`)
  await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
  bot.creative.startFlying()

  const confirmed = await scene('tp', 3000, 0, async () => {})
  h.result('tp.ticked_at_most_3', confirmed !== null && confirmed <= 3)
  const held = await scene('unconfirmed_tp', 3000, 3000, () => withoutConfirm(60))
  // World age arrives every 20 ticks, so the confirmation is held for 40 to 60 ticks.
  h.result('unconfirmed_tp.ticked_until_confirm', held !== null && held >= 30)

  await goTo(6000.5, 0.5)
  bot.removeAllListeners('kicked')
  bot.removeAllListeners('end')
  const ix = 6003.5; const iz = 0.5
  cmds(`summon item ${ix} ${Y} ${iz} ${itemNbt}`, `kick ${NAME}`)
  await h.sleep(4000)
  h.cmd('tick freeze')
  await h.sleep(500)
  const b2 = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username: NAME, version: '26.3', auth: 'offline' })
  await new Promise((resolve) => b2.once('spawn', resolve))
  await h.waitFor(() => itemNear(b2, ix, iz), 20000)
  const kicked = await queryAge(b2, ix, iz)
  h.cmd('tick unfreeze')
  console.log(`TIMING kick: item ticked ${kicked} times after the player left`)
  // Vanilla gives 1 (the kicked connection is dropped by ServerConnectionListener.tick, after the
  // next level tick), Pumpkin 0 (removed at once).
  h.result('kick.ticked_at_most_1', kicked !== null && kicked <= 1)
  b2.quit()
})
