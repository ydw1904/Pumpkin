// Ghasts and happy ghasts left alone should float around (`RandomFloatAroundGoal` with
// `GhastMoveControl`). Includes the bench mix layout where a row of happy ghasts is summoned
// overlapping a glass wall and each other. Reports how many of each group moved over 30 s.
const { make } = require('./survival-lib')
const h = make('Watcher')
const { bot } = h
const X = 9300; const Y = 200; const Z = 0

const groups = {}
const track = (label, name, near) => {
  groups[label] = Object.values(bot.entities)
    .filter((e) => e.name === name && near(e.position))
    .map((e) => ({ e, start: e.position.clone() }))
}
const moved = (label) => {
  const list = groups[label] || []
  return `${list.filter(({ e, start }) => e.position.distanceTo(start) > 1.0).length}/${list.length}`
}

h.start(async () => {
  h.cmd('difficulty normal')
  h.cmd(`gamemode spectator ${bot.username}`)
  await h.tp(X + 8, Y + 20, Z + 8)
  h.cmd(`fill ${X - 9} 199 ${Z - 9} ${X + 24} 214 ${Z + 24} glass hollow`)
  h.cmd(`fill ${X - 8} 200 ${Z - 8} ${X + 23} 202 ${Z + 23} water`)
  h.cmd(`fill ${X - 8} 203 ${Z - 8} ${X + 23} 203 ${Z + 23} glass`)
  await h.ticks(40)
  // The mix pack's happy ghast row: 3 apart and reaching into the wall at z = 24.
  for (let i = 0; i < 10; i++) h.cmd(`summon happy_ghast ${X - 6.5 + i * 3} 204 ${Z + 23.5} {PersistenceRequired:1b}`)
  // Room to float: inside the box away from the walls, and out in the open sky.
  h.cmd(`summon happy_ghast ${X + 8.5} 206 ${Z + 8.5} {PersistenceRequired:1b}`)
  for (let i = 0; i < 3; i++) h.cmd(`summon ghast ${X + 60.5 + i * 12} 230 ${Z + 0.5} {PersistenceRequired:1b}`)
  await h.waitFor(() => Object.values(bot.entities).filter((e) => e.name === 'ghast').length >= 3, 10000)
  await h.ticks(10)
  track('wall_row', 'happy_ghast', (p) => p.z > Z + 20)
  track('box_center', 'happy_ghast', (p) => p.z < Z + 20)
  track('open_ghast', 'ghast', () => true)
  await h.serverTicks(600)
  for (const label of Object.keys(groups)) h.result(`${label}_moved`, moved(label))
  h.cmd('kill @e[type=!player]')
})
