// Entity collision shapes (vanilla Entity.collide with Level.getEntityCollisions): items and
// armor stands land on boats and shulkers, and a sliding boat stops at an iron golem and at
// another boat.
const { make } = require('./survival-lib')
const h = make('Collider')
const { bot } = h
const X = 4000; const Y = 200; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m.toString()))

// The first reply with entity data, skipping other chat.
async function ask (cmd) {
  const c0 = chat.length
  const reply = () => chat.slice(c0).find((m) => m.includes('entity data') || m.includes('No entity'))
  bot.chat(cmd)
  if (!await h.waitFor(() => reply() !== undefined, 5000)) return '<no reply>'
  return reply()
}

async function pos (selector, digits = 3) {
  const reply = await ask(`/data get entity ${selector} Pos`)
  const nums = reply.replace(/^.*data: /, '').match(/-?\d+(\.\d+)?(E-?\d+)?/g)
  return nums ? nums.map((n) => Number(n)).map((n) => Number(n.toFixed(digits))) : null
}

async function clear () {
  for (const type of ['oak_boat', 'shulker', 'armor_stand', 'iron_golem', 'item']) h.cmd(`kill @e[type=${type}]`)
  await h.sleep(1000)
  // Killed armor stands and boats drop items.
  h.cmd('kill @e[type=item]')
  await h.sleep(500)
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('gamemode creative Collider')
  await h.tp(X + 0.5, Y + 1, Z + 7.5)
  await h.sleep(500)

  // An item dropped onto a boat on the floor rests on the boat.
  h.cmd(`summon oak_boat ${X + 0.5} ${Y + 1} ${Z + 0.5}`)
  await h.sleep(500)
  h.cmd(`summon item ${X + 0.5} ${Y + 4} ${Z + 0.5} {Item:{id:"minecraft:stone",count:1},PickupDelay:32767,Tags:["probe"]}`)
  await h.sleep(3000)
  h.result('item_on_boat.y', (await pos('@e[type=item,tag=probe,limit=1]'))[1])
  await clear()

  // An item and an armor stand dropped onto a shulker rest on its shell.
  h.cmd(`summon shulker ${X - 3} ${Y + 1} ${Z + 0.5} {NoAI:1b}`)
  await h.sleep(500)
  h.cmd(`summon item ${X - 2.5} ${Y + 4} ${Z + 0.5} {Item:{id:"minecraft:stone",count:1},PickupDelay:32767,Tags:["probe"]}`)
  h.cmd(`summon armor_stand ${X - 2.5} ${Y + 5} ${Z + 0.5}`)
  await h.sleep(3000)
  h.result('item_on_shulker.y', (await pos('@e[type=item,tag=probe,limit=1]'))[1])
  h.result('armor_stand_on_shulker.y', (await pos('@e[type=armor_stand,limit=1]'))[1])
  await clear()

  // An armor stand dropped onto a boat stands on it.
  h.cmd(`summon oak_boat ${X + 3.5} ${Y + 1} ${Z + 3.5}`)
  await h.sleep(500)
  h.cmd(`summon armor_stand ${X + 3.5} ${Y + 4} ${Z + 3.5}`)
  await h.sleep(3000)
  h.result('armor_stand_on_boat.y', (await pos('@e[type=armor_stand,limit=1]'))[1])
  await clear()

  // Boats pick up shulkers and other small mobs, so the obstacles are an iron golem, which is
  // too wide, and another boat.
  h.cmd(`fill ${X - 6} ${Y} ${Z - 4} ${X + 6} ${Y} ${Z - 4} blue_ice`)
  // Into an iron golem, which is pushable: the boat stops at it.
  h.cmd(`summon iron_golem ${X + 4.5} ${Y + 1} ${Z - 3.5} {NoAI:1b}`)
  await h.sleep(500)
  h.cmd(`summon oak_boat ${X + 1.5} ${Y + 1} ${Z - 3.5} {Motion:[1.0d,0.0d,0.0d]}`)
  await h.sleep(3000)
  h.result('boat_stops_at_golem.x', (await pos('@e[type=oak_boat,limit=1]', 1))[0])
  await clear()

  // Into another boat.
  h.cmd(`summon oak_boat ${X + 4.5} ${Y + 1} ${Z - 3.5}`)
  await h.sleep(500)
  h.cmd(`summon oak_boat ${X + 1.5} ${Y + 1} ${Z - 3.5} {Motion:[1.0d,0.0d,0.0d],Tags:["mover"]}`)
  await h.sleep(3000)
  h.result('boat_stops_at_boat.x', (await pos('@e[type=oak_boat,tag=mover,limit=1]', 1))[0])
  h.result('boat_stops_at_boat.other_x', (await pos('@e[type=oak_boat,tag=!mover,limit=1]', 1))[0])
  await clear()
})
