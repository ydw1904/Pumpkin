// Experience orbs: their value reaches the client, they fly to a player within 8 blocks, and a
// merged orb (Count) pays out once per merged orb. The client's total experience counts every
// point gained, which setting or spending levels doesn't lower.
const { make } = require('./survival-lib')
const h = make('Orbs')
const { bot } = h
const X = 1600; const Y = 200; const Z = 0
const orbValue = (e) => e.metadata ? e.metadata.find((x, i) => i >= 8 && typeof x === 'number') : undefined
const resetXp = async () => {
  h.cmd(`xp set ${bot.username} 0 levels`)
  h.cmd(`xp set ${bot.username} 0 points`)
  await h.waitFor(() => bot.experience.level === 0 && bot.experience.progress === 0, 3000)
}

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  await resetXp()
  h.cmd(`summon experience_orb ${X + 10.5} ${Y + 1} ${Z + 0.5} {Value:20}`)
  await h.waitFor(() => h.orbs().length > 0, 3000)
  h.result('orb_value_synced', h.orbs().map(orbValue).join(','))
  await h.ticks(40)
  h.result('far_orb_stays', h.orbs().length === 1 && bot.experience.progress === 0)

  h.cmd('kill @e[type=experience_orb]')
  await h.ticks(5)
  h.cmd(`summon experience_orb ${X + 6.5} ${Y + 1} ${Z + 0.5} {Value:20}`)
  h.result('near_orb_collected', await h.waitFor(() => bot.experience.level > 0, 10000))
  h.result('level_progress', `${bot.experience.level}/${Math.round(bot.experience.progress * 100)}`)

  await resetXp()
  h.cmd(`summon experience_orb ${X + 3.5} ${Y + 1} ${Z + 0.5} {Value:3,Count:3}`)
  await h.waitFor(() => bot.experience.progress > 0, 10000)
  await h.ticks(100)
  h.result('count3_level_progress', `${bot.experience.level}/${Math.round(bot.experience.progress * 100)}`)
  h.result('total_after_orbs', bot.experience.points)

  h.cmd(`xp add ${bot.username} 5 levels`)
  await h.waitFor(() => bot.experience.level === 5, 3000)
  h.result('total_after_levels', bot.experience.points)
  h.cmd(`xp add ${bot.username} -10 levels`)
  await h.waitFor(() => bot.experience.level === 0, 3000)
  await h.ticks(5)
  h.result('total_below_level_0', bot.experience.points)
  h.cmd(`xp add ${bot.username} 7 points`)
  await h.waitFor(() => bot.experience.progress > 0, 3000)
  h.result('total_after_points', bot.experience.points)
})
