// Drops arrows and snowballs onto entities that vanilla does or doesn't treat as pickable
// (`Entity.canBeHitByProjectile`): primed TNT, falling blocks, item frames and end crystals stop
// projectiles; XP orbs and interactions don't. Reports where each arrow ends up, whether the
// target survived and where the items it dropped landed.
const { make } = require('./survival-lib')
const h = make('Pickable')
const { bot } = h
const Y = 205

// [label, projectile, target summon, needs a block under the target]
const columns = [
  ['tnt', 'arrow', 'tnt ~ ~ ~ {fuse:2000,NoGravity:1b}'],
  ['falling_block', 'arrow', 'falling_block ~ ~ ~ {BlockState:{Name:"minecraft:stone"},NoGravity:1b}'],
  ['experience_orb', 'arrow', 'experience_orb ~ ~ ~ {Value:1,NoGravity:1b}'],
  ['interaction', 'arrow', 'interaction ~ ~ ~ {width:1f,height:1f}'],
  ['armor_stand', 'arrow', 'armor_stand ~ ~ ~ {NoGravity:1b}'],
  ['item_frame', 'arrow', 'item_frame ~ ~ ~ {Facing:1b,Item:{id:"minecraft:diamond",count:1}}', true],
  ['item_frame_snowball', 'snowball', 'item_frame ~ ~ ~ {Facing:1b,Item:{id:"minecraft:diamond",count:1}}', true],
  // Shot by the creative bot: vanilla drops nothing for a creative attacker.
  ['item_frame_creative', 'owned_arrow', 'item_frame ~ ~ ~ {Facing:1b,Item:{id:"minecraft:diamond",count:1}}', true]
]
// End crystals explode, so they go on the far side, one at a time, after the rest.
const crystals = [
  ['end_crystal', 'arrow', -24],
  ['end_crystal_snowball', 'snowball', -48]
]

const near = (name, x, r = 2) => Object.values(bot.entities).filter((e) => e.name === name && Math.abs(e.position.x - x) < r && Math.abs(e.position.z - 0.5) < r)
const targetName = (summon) => summon.split(' ')[0]

h.start(async () => {
  await h.arena(0, 199, 0, 4)
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('kill @e[type=!player]')
  await h.ticks(5)
  const xs = columns.map((_, i) => 8 + i * 6)
  columns.forEach(([, , summon, block], i) => {
    const x = xs[i]
    h.cmd(`fill ${x - 1} 200 -1 ${x + 1} 200 1 stone`)
    if (block) h.cmd(`fill ${x - 2} ${Y - 1} -2 ${x + 2} ${Y - 1} 2 stone`)
    h.cmd(`execute positioned ${x + 0.5} ${Y} 0.5 run summon ${summon}`)
  })
  await h.waitFor(() => columns.every(([, , summon], i) => near(targetName(summon), xs[i] + 0.5).length > 0), 5000)
  await h.ticks(10)
  const owner = bot.player.uuid.replace(/-/g, '').match(/.{8}/g).map((s) => parseInt(s, 16) | 0).join(',')
  columns.forEach(([, projectile], i) => {
    const [type, extra] = projectile === 'owned_arrow' ? ['arrow', `,Owner:[I;${owner}]`] : [projectile, '']
    h.cmd(`summon ${type} ${xs[i] + 0.5} ${Y + 5} 0.5 {Motion:[0.0,-1.5,0.0]${extra}}`)
  })
  await h.ticks(60)
  columns.forEach(([label, projectile, summon], i) => {
    const x = xs[i] + 0.5
    const arrows = near('arrow', x)
    if (projectile !== 'snowball') h.result(`${label}_arrow_y`, arrows.length ? arrows.map((a) => Math.floor(a.position.y)).join(',') : 'none')
    h.result(`${label}_target_alive`, near(targetName(summon), x).length > 0)
    h.result(`${label}_items`, near('item', x, 3).map((e) => Math.floor(e.position.y)).sort().join(',') || 'none')
  })
  for (const [label, projectile, x] of crystals) {
    h.cmd(`fill ${x - 1} 200 -1 ${x + 1} 200 1 stone`)
    h.cmd(`summon end_crystal ${x + 0.5} ${Y} 0.5 {ShowBottom:0b}`)
    await h.waitFor(() => near('end_crystal', x + 0.5).length > 0, 5000)
    await h.ticks(10)
    h.cmd(`summon ${projectile} ${x + 0.5} ${Y + 5} 0.5 {Motion:[0.0,-1.5,0.0]}`)
    await h.ticks(40)
    h.result(`${label}_target_alive`, near('end_crystal', x + 0.5).length > 0)
  }
})
