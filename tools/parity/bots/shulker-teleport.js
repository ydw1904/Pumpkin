// Shulker teleports (vanilla Shulker.teleportSomewhere): a shulker with nothing to attach to
// teleports onto a stone layer below it, never onto bedrock (`shulker_does_not_teleport_to`)
// and never outside the world border.
const { make } = require('./survival-lib')
const h = make('Shulkerer')
const { bot } = h
const X = 4200; const Y = 200; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m.toString()))

async function ask (cmd) {
  const c0 = chat.length
  const reply = () => chat.slice(c0).find((m) => m.includes('entity data') || m.includes('No entity'))
  bot.chat(cmd)
  if (!await h.waitFor(() => reply() !== undefined, 5000)) return '<no reply>'
  return reply()
}

async function pos (selector) {
  const reply = await ask(`/data get entity ${selector} Pos`)
  const nums = reply.replace(/^.*data: /, '').match(/-?\d+(\.\d+)?(E-?\d+)?/g)
  return nums ? nums.map((n) => Number(n)) : null
}

// A floating shulker above a 17x17 layer of `block` four blocks below it, with the middle
// `hole` wide square of the layer removed.
async function floating (y, block, hole = 0) {
  h.cmd(`fill ${X - 8} ${y - 4} ${Z - 8} ${X + 8} ${y - 4} ${Z + 8} ${block}`)
  if (hole > 0) h.cmd(`fill ${X - hole} ${y - 4} ${Z - hole} ${X + hole} ${y - 4} ${Z + hole} air`)
  await h.sleep(500)
  h.cmd(`summon shulker ${X + 0.5} ${y} ${Z + 0.5} {PersistenceRequired:1b}`)
  await h.sleep(5000)
  const p = await pos('@e[type=shulker,limit=1]')
  h.cmd('kill @e[type=shulker]')
  await h.sleep(500)
  return p
}

h.start(async () => {
  await h.arena(X, Y, Z, 8)
  h.cmd('gamemode creative Shulkerer')
  await h.sleep(500)

  // On stone it lands on top of or under the layer.
  const onStone = await floating(Y + 60, 'stone')
  h.result('stone.moved', onStone[1] !== Y + 60)
  h.result('stone.by_layer', onStone[1] === Y + 57 || onStone[1] === Y + 55)

  // Bedrock is never a target.
  const onBedrock = await floating(Y + 40, 'bedrock')
  h.result('bedrock.stays', onBedrock.join(','))

  // Inside a 3 wide border most blocks to attach to are outside it: it stays inside.
  h.cmd(`worldborder center ${X + 0.5} ${Z + 0.5}`)
  h.cmd('worldborder set 3')
  await h.sleep(500)
  const bordered = await floating(Y + 20, 'stone', 1)
  h.result('border.inside', bordered[0] >= X - 1 && bordered[0] < X + 2 && bordered[2] >= Z - 1 && bordered[2] < Z + 2)
  h.cmd('worldborder set 59999968')
  h.cmd('worldborder center 0 0')
})
