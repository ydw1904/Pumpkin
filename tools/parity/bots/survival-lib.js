// Helpers for the survival checks: a bot, console commands, a stone floor in the sky, inventory
// summaries. Not a check itself.
const fs = require('fs')
const mineflayer = require('mineflayer')
const { Vec3 } = require('vec3')

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function make (username, timeoutMs = 600000) {
  const bot = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username, version: '26.3', auth: 'offline' })
  const h = { bot, Vec3, sleep }
  h.result = (key, value) => console.log(`RESULT ${key} ${value}`)
  h.cmd = (c) => fs.appendFileSync(process.env.CMDS, c + '\n')
  h.ticks = (n) => bot.waitForTicks(n)
  // Polls on wall-clock time: bot ticks stop while the bot stands in a chunk it hasn't received.
  h.waitFor = async (pred, ms = 5000) => {
    const end = Date.now() + ms
    while (Date.now() < end) {
      try { if (pred()) return true } catch (e) {}
      await sleep(50)
    }
    return false
  }
  h.block = (x, y, z) => bot.blockAt(new Vec3(x, y, z))
  h.blockName = (x, y, z) => { const b = h.block(x, y, z); return b ? b.name : 'unloaded' }
  h.chunksLoaded = () => {
    const { x, z } = bot.entity.position
    for (let dx = -1; dx <= 1; dx++) for (let dz = -1; dz <= 1; dz++) if (!bot.world.getColumnAt(new Vec3(x + dx * 16, 0, z + dz * 16))) return false
    return true
  }
  h.tp = async (x, y, z, yaw = 0, pitch = 0) => {
    h.cmd(`tp ${username} ${x} ${y} ${z} ${yaw} ${pitch}`)
    await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(x, y, z)) < 1.5, 10000)
    await h.waitFor(h.chunksLoaded, 120000)
  }
  // Item counts by name, offhand included.
  h.inv = () => {
    const c = {}
    const stacks = bot.inventory.items().concat(bot.inventory.slots[45] ? [bot.inventory.slots[45]] : [])
    for (const it of stacks) c[it.name] = (c[it.name] || 0) + it.count
    return c
  }
  h.invStr = () => { const c = h.inv(); return Object.keys(c).sort().map(k => `${k}x${c[k]}`).join(',') || 'empty' }
  h.slots = () => bot.inventory.slots.map((it, i) => it ? `${i}:${it.name}` : null).filter(Boolean).join(',') || 'empty'
  h.clear = async () => { h.cmd(`clear ${username}`); await h.waitFor(() => bot.inventory.items().length === 0, 3000); await h.ticks(2) }
  h.give = async (item, n = 1) => {
    const name = item.split('[')[0]
    const before = h.inv()[name] || 0
    h.cmd(`give ${username} ${item} ${n}`)
    await h.waitFor(() => (h.inv()[name] || 0) >= before + n, 5000)
  }
  h.orbs = () => Object.values(bot.entities).filter(e => e.name === 'experience_orb')
  // A raw window click with a stale state id: the server applies it and answers with the whole
  // window, so what the bot sees afterwards is the server's result, not mineflayer's prediction.
  h.click = async (slot, button = 0, mode = 0) => {
    const Item = require('prismarine-item')(bot.registry)
    const window = bot.currentWindow || bot.inventory
    const synced = new Promise((resolve) => {
      bot.once(`setWindowItems:${window.id}`, resolve)
      setTimeout(resolve, 3000)
    })
    bot._client.write('window_click', {
      windowId: window.id, stateId: -1, slot, mouseButton: button, mode, changedSlots: [], cursorItem: Item.toNotch(window.selectedItem)
    })
    await synced
    await h.ticks(1)
  }
  h.stackStr = (it) => it ? `${it.name}*${it.count}` : 'empty'
  // Non-empty slots of the open window (or the inventory) as `slot:item*count`, then the cursor.
  h.win = (from = 0, to) => {
    const window = bot.currentWindow || bot.inventory
    const end = to === undefined ? window.slots.length : to
    const parts = []
    for (let i = from; i < end; i++) if (window.slots[i]) parts.push(`${i}:${h.stackStr(window.slots[i])}`)
    if (window.selectedItem) parts.push(`cursor:${h.stackStr(window.selectedItem)}`)
    return parts.join(',') || 'empty'
  }
  // Fills inventory slots by vanilla index (0-8 hotbar, 9-35 main) from a {index: 'item count'} map.
  h.fill = async (map) => {
    await h.clear()
    for (const [i, spec] of Object.entries(map)) {
      const [item, n] = spec.split(' ')
      h.cmd(`item replace entity ${username} container.${i} with ${item} ${n || 1}`)
    }
    await h.waitFor(() => Object.keys(map).every(i => bot.inventory.slots[Number(i) < 9 ? Number(i) + 36 : Number(i)]), 5000)
    await h.ticks(2)
  }
  // Waits until the server's world age has moved on by `n` ticks, so slow servers aren't
  // measured against the bot's own clock. Updates arrive every 20 ticks.
  h.serverTicks = async (n) => {
    const start = bot.time.age
    await h.waitFor(() => bot.time.age - start >= n, n * 500)
  }
  // Hovers in creative flight until the chunks are there, then builds a stone floor under the bot.
  h.arena = async (x, y, z, r = 6) => {
    h.cmd(`gamemode creative ${username}`)
    await h.waitFor(() => bot.game.gameMode === 'creative', 5000)
    h.cmd(`tp ${username} ${x + 0.5} ${y + 1} ${z + 0.5}`)
    await h.waitFor(() => bot.entity.position.distanceTo(new Vec3(x + 0.5, y + 1, z + 0.5)) < 1.5, 10000)
    bot.creative.startFlying()
    await h.waitFor(h.chunksLoaded, 180000)
    for (let i = 0; i < 20; i++) {
      h.cmd(`fill ${x - r} ${y} ${z - r} ${x + r} ${y} ${z + r} stone`)
      h.cmd(`fill ${x - r} ${y + 1} ${z - r} ${x + r} ${y + 6} ${z + r} air`)
      if (await h.waitFor(() => h.blockName(x + r, y, z + r) === 'stone' && h.blockName(x - r, y, z - r) === 'stone', 3000)) break
    }
    bot.creative.stopFlying()
    h.cmd(`tp ${username} ${x + 0.5} ${y + 1} ${z + 0.5}`)
    h.cmd(`gamemode survival ${username}`)
    await h.waitFor(() => bot.game.gameMode === 'survival', 5000)
    await h.ticks(10)
    h.result('arena_ok', h.blockName(x, y, z) === 'stone' && bot.entity.position.y > y)
  }
  h.respawnAfterDeath = async () => {
    await h.waitFor(() => bot.health <= 0, 5000)
    await sleep(1500)
    bot.respawn()
    await sleep(2000)
  }
  h.start = (fn) => {
    bot.on('kicked', (r) => { h.result('kicked', JSON.stringify(r)); process.exit(1) })
    bot.on('error', (e) => console.log('ERROR', e.message))
    setTimeout(() => { h.result('finished', 'timeout'); process.exit(1) }, timeoutMs)
    bot.once('spawn', async () => {
      try {
        h.cmd(`op ${username}`)
        h.cmd('gamerule spawn_mobs false')
        await h.waitFor(() => bot.health !== undefined, 5000)
        await h.waitFor(h.chunksLoaded, 120000)
        await h.ticks(10)
        await fn(h)
        h.result('finished', 'true')
      } catch (e) {
        h.result('exception', e.message.replace(/\s+/g, ' ').slice(0, 200))
      }
      bot.quit()
      setTimeout(() => process.exit(0), 1000)
    })
  }
  return h
}

module.exports = { make }
