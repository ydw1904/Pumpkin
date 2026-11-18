// Thrown ender pearls hold their chunk with vanilla's `ENDER_PEARL` ticket while their owner is
// online, so a pearl keeps flying past the simulation distance. Through a portal it arrives with
// a `PORTAL` ticket and keeps flying in the other dimension, where no player loads anything.
// pumpkin-settings: allow_nether=true
const { make } = require('./survival-lib')
const h = make('Pearler')
const { bot, Vec3 } = h
const X = 5200; const Y = 200; const Z = 0
// The nether side, near X / 8.
const NX = 650
// Past the simulation distance (6 chunks) of a player at Z.
const WALL = 130

const dimName = () => String(bot.game.dimension).replace('minecraft:', '')
const fill = (dim, a, b, block) => h.cmd(`execute in minecraft:${dim} run fill ${a.join(' ')} ${b.join(' ')} ${block}`)
const ownerTag = () => `Owner:[I;${bot.player.uuid.replace(/-/g, '').match(/.{8}/g).map((s) => parseInt(s, 16) | 0).join(',')}]`

// Forceloads an area, waits until it is loaded, builds in it and lets it go again.
async function build (dim, x1, z1, x2, z2, work) {
  h.cmd(`execute in minecraft:${dim} run forceload add ${x1} ${z1} ${x2} ${z2}`)
  const sx = Math.floor(bot.entity.position.x); const sy = Math.floor(bot.entity.position.y) + 3; const sz = Math.floor(bot.entity.position.z)
  for (let i = 0; i < 120 && h.blockName(sx, sy, sz) !== 'glowstone'; i++) {
    h.cmd(`execute in minecraft:${dim} if loaded ${x1} 64 ${z1} if loaded ${x2} 64 ${z2} run execute in minecraft:overworld run setblock ${sx} ${sy} ${sz} glowstone`)
    await h.sleep(1000)
  }
  h.cmd(`setblock ${sx} ${sy} ${sz} air`)
  work()
  await h.ticks(10)
  h.cmd(`execute in minecraft:${dim} run forceload remove ${x1} ${z1} ${x2} ${z2}`)
  await h.ticks(60)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  await build('overworld', X - 4, Z + WALL - 2, X + 4, Z + WALL + 2, () => {
    fill('overworld', [X - 3, Y - 2, Z + WALL], [X + 3, Y + 4, Z + WALL], 'stone')
  })
  h.cmd(`summon ender_pearl ${X + 0.5} ${Y + 1.5} ${Z + 2.5} {Motion:[0.0,0.0,1.6],NoGravity:1b,${ownerTag()}}`)
  await h.waitFor(() => bot.entity.position.z > Z + 50, 20000)
  h.result('far_flight', bot.entity.position.z > Z + 50 ? `landed z=${Math.floor(bot.entity.position.z) - Z}` : 'stuck')

  // A closed netherrack corridor along Z behind a portal along X, built and then unloaded.
  await h.arena(X, Y, Z, 8)
  await build('the_nether', NX - 8, -12, NX + 8, 40, () => {
    fill('the_nether', [NX - 4, 58, -8], [NX + 5, 68, 34], 'netherrack')
    fill('the_nether', [NX - 2, 60, -6], [NX + 3, 66, 32], 'air')
    fill('the_nether', [NX - 1, 60, 0], [NX + 2, 64, 0], 'obsidian')
    fill('the_nether', [NX, 61, 0], [NX + 1, 63, 0], 'nether_portal[axis=x]')
  })
  fill('overworld', [X - 1, Y + 1, Z], [X + 2, Y + 5, Z], 'obsidian')
  fill('overworld', [X, Y + 2, Z], [X + 1, Y + 4, Z], 'nether_portal[axis=x]')
  await h.ticks(20)
  h.result('portal', h.blockName(X, Y + 2, Z))
  h.cmd(`summon ender_pearl ${X + 0.5} ${Y + 3} ${Z - 2.5} {Motion:[0.0,0.0,1.0],NoGravity:1b,${ownerTag()}}`)
  await h.waitFor(() => dimName() === 'the_nether', 20000)
  await h.waitFor(() => bot.entity && bot.entity.position && h.chunksLoaded(), 60000)
  const pos = bot.entity.position
  h.result('through_portal', dimName() === 'the_nether' ? `${dimName()} z=${Math.floor(pos.z)}` : dimName())
  h.cmd('execute in minecraft:the_nether run kill @e[type=ender_pearl]')
})
