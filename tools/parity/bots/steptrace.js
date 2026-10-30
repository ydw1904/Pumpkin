// Tick-by-tick traces: freeze the game, build a scene, then `/tick step 1` at a time and read
// a fixed set of probes after every step. Prints `RESULT t<tick>.<probe> <value>` lines, so
// bot_parity.py can point at the first tick where vanilla and Pumpkin disagree.
//
// A trace check calls `trace({ name, setup, probes, ticks })`:
//   setup:  commands run while frozen, before tick 0 (the bot stands at the origin of a
//           stone platform at PLATFORM_Y).
//   probes: { key: '/command' } read from the server's chat reply, or
//           { key: { block: [x, y, z] } } read from the bot's own view of the world (what a
//           client was sent), or
//           { key: { server: [x, y, z], states: ['air', 'piston[extended=true]', ...] } }, the
//           first state `execute if block` accepts on the server ('other' if none).
//   ticks:  how many single steps to trace.
// Command replies are normalised (see `normalise`) so the trace compares values, not formatting.
//
// Pumpkin runs chat commands as async tasks next to the tick, and its world tick advances game
// time first, so a reading taken as soon as game time moves can land mid-tick. Probes are read
// until two passes in a row agree; while frozen that only happens once the step is complete.
const fs = require('fs')
const mineflayer = require('mineflayer')
const { Vec3 } = require('vec3')

const PLATFORM_Y = 200
const result = (key, value) => console.log(`RESULT ${key} ${value}`)
const consoleCmd = (cmd) => fs.appendFileSync(process.env.CMDS, cmd + '\n')
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

// Drops the "<name> has the following ... data:" prefix and writes SNBT numbers one way, so
// `0.0d` (vanilla) and `0d` (Pumpkin) compare equal. Formatting differences are separate bugs.
function normalise (reply) {
  return reply
    .replace(/^.*? has the following (?:entity|block) data: /, '')
    .replace(/(-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)([bsLfd])?\b/g, (_, n, suffix) => String(Number(n)) + (suffix || ''))
}

async function waitFor (predicate, timeoutMs, stepMs = 50) {
  const end = Date.now() + timeoutMs
  while (Date.now() < end) {
    if (predicate()) return true
    await sleep(stepMs)
  }
  return false
}

function trace ({ name, setup, probes, ticks }) {
  const bot = mineflayer.createBot({
    host: '127.0.0.1',
    port: Number(process.env.PORT),
    username: 'Trace',
    version: '26.3',
    auth: 'offline'
  })
  const chat = []
  bot.on('message', (m) => chat.push(m.toString()))
  bot.on('kicked', (reason) => { result('kicked', JSON.stringify(reason)); process.exit(1) })
  bot.on('error', (err) => console.log('ERROR', err.message))
  setTimeout(() => { result('finished', 'timeout'); process.exit(1) }, 280000)

  // Runs one command and returns its first reply line. Commands go one at a time because
  // Pumpkin doesn't guarantee the reply order of commands sent together.
  async function ask (cmd, timeoutMs = 5000) {
    const c0 = chat.length
    bot.chat(cmd)
    if (!await waitFor(() => chat.length > c0, timeoutMs, 10)) return '<no reply>'
    await sleep(30) // let multi-line replies finish arriving
    return chat[c0]
  }

  async function gameTime () {
    const reply = await ask('/time query gametime')
    const match = reply.match(/(-?\d+)/)
    return match ? Number(match[1]) : null
  }

  function readBlock ([x, y, z]) {
    const b = bot.blockAt(new Vec3(x, y, z))
    if (!b) return 'unloaded'
    const props = Object.entries(b.getProperties()).map(([k, v]) => `${k}=${v}`).join(',')
    return props ? `${b.name}[${props}]` : b.name
  }

  async function serverBlock ([x, y, z], states) {
    for (const state of states) {
      if ((await ask(`/execute if block ${x} ${y} ${z} ${state}`)).startsWith('Test passed')) return state
    }
    return 'other'
  }

  async function readProbes () {
    const values = {}
    for (const [key, probe] of Object.entries(probes)) {
      if (typeof probe === 'string') values[key] = normalise(await ask(probe))
      else if (probe.block) values[key] = readBlock(probe.block)
      else values[key] = await serverBlock(probe.server, probe.states)
    }
    return values
  }

  async function settledProbes () {
    let last = await readProbes()
    for (let i = 0; i < 10; i++) {
      await sleep(60)
      const next = await readProbes()
      if (JSON.stringify(next) === JSON.stringify(last)) return last
      last = next
    }
    result('unsettled', 'true')
    return last
  }

  bot.once('spawn', async () => {
    try {
      await sleep(1000)
      consoleCmd(`op ${bot.username}`)
      await sleep(1000)
      await ask('/gamemode creative')
      await ask('/gamerule spawn_mobs false')
      bot.creative.startFlying()
      await ask(`/tp ${bot.username} 0.5 ${PLATFORM_Y + 1} 0.5`)
      const loaded = await waitFor(() => [[0, 0], [-16, -16], [16, 16]].every(([x, z]) => bot.blockAt(new Vec3(x, PLATFORM_Y, z)) !== null), 120000, 100)
      result('chunks_loaded', loaded)
      await ask(`/fill -12 ${PLATFORM_Y} -12 12 ${PLATFORM_Y} 12 stone`)
      await ask(`/fill -12 ${PLATFORM_Y + 1} -12 12 ${PLATFORM_Y + 20} 12 air`)
      await ask('/kill @e[type=!player]')
      await sleep(500)

      await ask('/tick freeze')
      for (const [i, cmd] of setup.entries()) result(`setup.${i}`, `${cmd} -> ${await ask(cmd)}`)
      let time = await gameTime()
      let tick = 0
      const report = async () => {
        const values = await settledProbes()
        for (const [key, value] of Object.entries(values)) result(`t${String(tick).padStart(3, '0')}.${key}`, value)
      }
      await report()
      for (tick = 1; tick <= ticks; tick++) {
        await ask('/tick step 1')
        let now = time
        for (let i = 0; i < 100 && now === time; i++) now = await gameTime()
        if (now !== time + 1) result(`t${String(tick).padStart(3, '0')}.game_time_advance`, now === null ? 'unknown' : now - time)
        time = now
        await report()
      }
      await ask('/tick unfreeze')
      result('finished', name)
    } catch (e) {
      console.log('ERROR', e.stack)
    }
    bot.quit()
    setTimeout(() => process.exit(0), 500)
  })
}

module.exports = { trace, PLATFORM_Y }
