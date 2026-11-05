// Shulker shells (vanilla Shulker.onPeekAmountChange, setRawPeekAmount, hitByShulkerBullet): an
// opening shell grows its box and lifts what stands on it, closing adds 20 armor, and an open
// shulker hit by another's bullet teleports away and leaves a copy behind.
const { make } = require('./survival-lib')
const h = make('Peeker')
const { bot } = h
const X = 4400; const Y = 200; const Z = 0
const PEEK = 17

const chat = []
bot.on('message', (m) => chat.push(m.toString()))

async function ask (cmd, pattern) {
  const c0 = chat.length
  const reply = () => chat.slice(c0).find((m) => pattern.test(m))
  bot.chat(cmd)
  if (!await h.waitFor(() => reply() !== undefined, 5000)) return '<no reply>'
  return reply()
}

async function y (selector) {
  const reply = await ask(`/data get entity ${selector} Pos`, /entity data|No entity/)
  const nums = reply.replace(/^.*data: /, '').match(/-?\d+(\.\d+)?(E-?\d+)?/g)
  return nums ? Number(nums[1]) : null
}

async function armor () {
  const reply = await ask('/attribute @e[type=shulker,limit=1] minecraft:armor get', /attribute|No entity|entities/)
  const nums = reply.match(/-?\d+(\.\d+)?/g)
  return nums ? Number(nums[nums.length - 1]) : null
}

const shulkers = () => Object.values(bot.entities).filter((e) => e.name === 'shulker')

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('difficulty normal')
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd(`tp ${bot.username} ${X + 6.5} ${Y + 1} ${Z + 6.5}`)
  await h.ticks(20)

  // An armor stand on a closed floor shulker stands on its lid, one block up, and rides the lid
  // up to two blocks when the shell opens all the way.
  h.cmd(`summon shulker ${X + 0.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  h.cmd(`summon armor_stand ${X + 0.5} ${Y + 2.5} ${Z + 0.5}`)
  await h.serverTicks(40)
  h.result('push.closed', (await y('@e[type=armor_stand,limit=1]')) - Y)
  h.cmd('data merge entity @e[type=shulker,limit=1] {Peek:100b}')
  await h.serverTicks(40)
  h.result('push.open', (await y('@e[type=armor_stand,limit=1]')) - Y)
  h.cmd('kill @e[type=!player]')
  await h.ticks(20)

  // A new shulker has no covered armor until it first closes.
  h.cmd(`summon shulker ${X + 0.5} ${Y + 1} ${Z + 0.5} {PersistenceRequired:1b}`)
  await h.waitFor(() => shulkers().length === 1, 5000)
  await h.ticks(10)
  h.result('armor.new', await armor())
  // It opens to shoot a survival player and closes once the player turns creative.
  h.cmd(`effect give ${bot.username} resistance infinite 4 true`)
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 4.5}`)
  h.cmd(`gamemode survival ${bot.username}`)
  h.result('armor.opened', await h.waitFor(() => (shulkers()[0]?.metadata[PEEK] ?? 0) > 0, 10000))
  h.cmd(`gamemode creative ${bot.username}`)
  // The peek goal may open it again right away, so keep the highest value seen.
  let highest = 0
  for (let i = 0; i < 10; i++) highest = Math.max(highest, await armor())
  h.result('armor.closed', highest)
  h.cmd('kill @e[type=!player]')
  h.cmd(`effect clear ${bot.username} levitation`)
  await h.ticks(20)

  // A glass tunnel along x: the bullets of the first shulker fly through the second one on
  // their way to the player at the far end.
  h.cmd(`fill ${X - 4} ${Y + 1} ${Z - 1} ${X + 5} ${Y + 3} ${Z + 1} glass`)
  h.cmd(`fill ${X - 4} ${Y + 1} ${Z} ${X + 5} ${Y + 2} ${Z} air`)
  h.cmd(`tp ${bot.username} ${X + 4.5} ${Y + 1} ${Z + 0.5}`)
  await h.ticks(10)
  h.cmd(`summon shulker ${X - 3.5} ${Y + 1} ${Z + 0.5} {PersistenceRequired:1b}`)
  h.cmd(`summon shulker ${X + 0.5} ${Y + 1} ${Z + 0.5} {PersistenceRequired:1b}`)
  await h.waitFor(() => shulkers().length === 2, 5000)
  h.cmd(`gamemode survival ${bot.username}`)
  h.cmd(`effect give ${bot.username} resistance infinite 4 true`)
  h.cmd(`effect give ${bot.username} regeneration infinite 4 true`)
  await h.serverTicks(600)
  h.result('dup.copied', shulkers().length > 2)
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('kill @e[type=shulker]')
  h.cmd('kill @e[type=shulker_bullet]')
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(20)

  // An open shulker keeps its peek through /tp (snapTo, not Shulker.setPos), and a closed
  // shell turns away every AbstractArrow, tridents included.
  h.cmd(`summon shulker ${X + 0.5} ${Y + 4} ${Z + 4.5} {NoAI:1b,PersistenceRequired:1b,Peek:100b,Tags:["tp"]}`)
  await h.serverTicks(20)
  h.cmd(`tp @e[tag=tp,limit=1] ${X + 2.5} ${Y + 4} ${Z + 4.5}`)
  await h.serverTicks(5)
  h.result('tp.peek', await ask('/data get entity @e[tag=tp,limit=1] Peek', /entity data|No entity/))
  h.cmd('kill @e[type=!player]')
  h.cmd(`summon shulker ${X + 2.5} ${Y + 1} ${Z + 4.5} {NoAI:1b,PersistenceRequired:1b,Tags:["closed"]}`)
  await h.serverTicks(20)
  h.cmd(`summon trident ${X + 2.5} ${Y + 5} ${Z + 4.5} {Motion:[0.0,-1.0,0.0]}`)
  await h.serverTicks(20)
  h.result('trident.health', await ask('/data get entity @e[tag=closed,limit=1] Health', /entity data|No entity/))
  h.cmd('kill @e[type=!player]')
})
