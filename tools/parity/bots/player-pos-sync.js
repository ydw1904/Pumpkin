// Where a watching player sees another player after jumping, stepping up blocks and slabs,
// riding, sleeping and an ender pearl. Vanilla's tracker sends relative moves against the
// last position it sent and a full position sync on every ground change and after riding, so
// the watcher's copy ends up where the mover really is: the drift is 0.00 after each scene.
const mineflayer = require('mineflayer')
const { make } = require('./survival-lib')
const h = make('Watcher')
const { bot } = h
const X = 5300; const Y = 200; const Z = 0

let mover = null
const seen = () => Object.values(bot.entities).find((e) => e.username === 'Mover')

function drift () {
  const e = seen()
  if (!e) return 'unseen'
  const d = e.position.minus(mover.entity.position)
  return Math.max(Math.abs(d.x), Math.abs(d.y), Math.abs(d.z)).toFixed(2)
}

async function settle () {
  mover.clearControlStates()
  await h.ticks(30)
}

async function walk (yaw, ticks, jump = false) {
  await mover.look(yaw, 0, true)
  mover.setControlState('forward', true)
  mover.setControlState('jump', jump)
  await h.ticks(ticks)
  await settle()
}

// A teleport to the middle of the floor syncs both copies, so each scene starts clean.
async function reset () {
  h.cmd(`tp Mover ${X + 0.5} ${Y + 1} ${Z + 0.5}`)
  await h.waitFor(() => mover.entity.position.distanceTo(new h.Vec3(X + 0.5, Y + 1, Z + 0.5)) < 0.1, 5000)
  await settle()
}

async function scene (name, fn) {
  await reset()
  h.result(`${name}_start`, drift())
  await fn()
  await settle()
  h.result(`${name}_drift`, drift())
}

h.start(async () => {
  await h.arena(X, Y, Z, 9)
  mover = mineflayer.createBot({ host: '127.0.0.1', port: Number(process.env.PORT), username: 'Mover', version: '26.3', auth: 'offline' })
  await new Promise((resolve) => mover.once('spawn', resolve))
  h.cmd('op Mover')
  h.cmd('gamemode survival Mover')
  h.cmd('gamerule fall_damage false')
  await h.waitFor(() => seen(), 10000)
  await reset()

  await scene('jumps', async () => {
    mover.setControlState('jump', true)
    await h.ticks(120)
  })
  await scene('walk_jump', async () => {
    await walk(Math.PI / 2, 25, true)
    await walk(-Math.PI / 2, 25, true)
  })

  // A one block step to the east and a slab step to the west.
  h.cmd(`fill ${X + 3} ${Y + 1} ${Z - 2} ${X + 8} ${Y + 1} ${Z + 2} stone`)
  h.cmd(`fill ${X - 3} ${Y + 1} ${Z - 2} ${X - 8} ${Y + 1} ${Z + 2} smooth_stone_slab`)
  await h.waitFor(() => h.blockName(X - 8, Y + 1, Z + 2) === 'smooth_stone_slab', 5000)
  await scene('block_up', () => walk(-Math.PI / 2, 20, true))
  await scene('block_up_down', async () => {
    await walk(-Math.PI / 2, 20, true)
    await walk(Math.PI / 2, 20)
  })
  await scene('slab_up', () => walk(Math.PI / 2, 20))
  await scene('slab_up_down', async () => {
    await walk(Math.PI / 2, 20)
    await walk(-Math.PI / 2, 20)
  })
  h.cmd(`fill ${X - 8} ${Y + 1} ${Z - 2} ${X + 8} ${Y + 1} ${Z + 2} air`)
  await h.waitFor(() => h.blockName(X - 8, Y + 1, Z + 2) === 'air', 5000)

  for (const [name, summon] of [['boat', 'oak_boat'], ['horse', 'horse']]) {
    await scene(name, async () => {
      h.cmd(`summon ${summon} ${X + 0.5} ${Y + 1} ${Z + 3.5} {NoAI:1b,Tags:["ride"]}`)
      await h.waitFor(() => Object.values(bot.entities).some((e) => e.name === summon), 5000)
      h.cmd('ride Mover mount @e[tag=ride,limit=1]')
      await h.ticks(20)
      h.cmd(`tp @e[tag=ride] ${X + 3.5} ${Y + 1} ${Z + 3.5}`)
      await h.ticks(20)
      h.cmd('ride Mover dismount')
      await h.ticks(20)
      await walk(0, 10)
      h.cmd('kill @e[tag=ride]')
    })
  }

  await scene('bed', async () => {
    h.cmd('time set 18000')
    h.cmd(`setblock ${X + 2} ${Y + 1} ${Z} red_bed[facing=east,part=foot]`)
    h.cmd(`setblock ${X + 3} ${Y + 1} ${Z} red_bed[facing=east,part=head]`)
    await h.waitFor(() => h.blockName(X + 3, Y + 1, Z) === 'red_bed', 5000)
    await mover.sleep(mover.blockAt(new h.Vec3(X + 2, Y + 1, Z))).catch((e) => h.result('sleep_error', e.message))
    await h.ticks(40)
    await mover.wake().catch((e) => h.result('wake_error', e.message))
    await h.ticks(20)
    await walk(Math.PI, 10)
    h.cmd(`fill ${X + 2} ${Y + 1} ${Z} ${X + 3} ${Y + 1} ${Z} air`)
    h.cmd('time set 1000')
  })

  await scene('pearl', async () => {
    h.cmd('give Mover ender_pearl 1')
    await h.waitFor(() => mover.inventory.items().some((i) => i.name === 'ender_pearl'), 5000)
    await mover.equip(mover.inventory.items().find((i) => i.name === 'ender_pearl'), 'hand')
    await mover.look(0, -0.7, true)
    mover.activateItem()
    await h.ticks(60)
    await walk(Math.PI, 10)
  })

  await scene('tp_small', async () => {
    h.cmd(`tp Mover ${X + 2.5} ${Y + 1} ${Z - 2.5}`)
    await h.waitFor(() => mover.entity.position.distanceTo(new h.Vec3(X + 2.5, Y + 1, Z - 2.5)) < 0.1, 5000)
    await walk(0, 10)
  })
  mover.quit()
})
