// Pandas: feeding bamboo to an adult that is already in love makes it sit down and eat, a worried
// panda sits through a thunderstorm and ignores bamboo while scared, and a panda picks up cake
// from the ground and sits down with it. Flags and the eat counter are read from entity data.
const { make } = require('./survival-lib')
const h = make('Keeper')
const { bot } = h
const X = 9200; const Y = 200; const Z = 0
// Panda entity data: 20 is the eat counter, 23 the sneeze/roll/sit/on-back flags.
const EAT_COUNTER = 20; const FLAGS = 23

function panda () {
  return bot.nearestEntity((e) => e.name === 'panda')
}

function state (label) {
  const p = panda()
  if (!p) return h.result(label, 'no panda')
  const held = p.equipment && p.equipment[0] ? p.equipment[0].name : 'empty'
  h.result(label, `flags=${p.metadata[FLAGS] ?? 0} eating=${(p.metadata[EAT_COUNTER] ?? 0) > 0} held=${held}`)
}

async function interact () {
  bot.setQuickBarSlot(0)
  bot._client.write('use_entity', { target: panda().id, hand: 0, location: { x: 0, y: 0.5, z: 0 }, sneaking: false })
  await h.ticks(4)
}

async function summon (genes, extra = '') {
  h.cmd('kill @e[type=panda]')
  h.cmd('kill @e[type=item]')
  await h.ticks(5)
  h.cmd(`summon panda ${X + 2.5} ${Y + 1} ${Z + 0.5} {MainGene:"${genes}",HiddenGene:"${genes}"${extra}}`)
  await h.waitFor(panda, 5000)
  await h.ticks(10)
}

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('weather clear')
  h.cmd('gamerule mob_griefing true')

  await summon('normal')
  await h.give('bamboo', 3)
  await interact()
  state('feed.in_love')
  h.result('feed.in_love.bamboo', h.inv().bamboo || 0)
  await interact()
  await h.ticks(10)
  state('feed.sitting')
  h.result('feed.sitting.bamboo', h.inv().bamboo || 0)
  // A sitting panda takes nothing more.
  await interact()
  h.result('feed.again.bamboo', h.inv().bamboo || 0)

  // Thunder level climbs for a while before the world counts as thundering.
  h.cmd('weather thunder')
  await summon('worried')
  await h.ticks(200)
  state('worried.thunder')
  await interact()
  state('worried.scared_feed')
  h.result('worried.scared_feed.bamboo', h.inv().bamboo || 0)
  h.cmd('weather clear')
  await h.ticks(200)
  state('worried.clear')

  // NBT without CanPickUpLoot loads it as false, as on any mob.
  await summon('normal', ',CanPickUpLoot:1b')
  h.cmd(`summon item ${X + 2.5} ${Y + 1} ${Z + 0.5} {Item:{id:"minecraft:cake",count:1},PickupDelay:0}`)
  await h.ticks(40)
  state('pickup.cake')
  h.cmd('kill @e[type=panda]')
})
