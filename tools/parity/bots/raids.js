// Raids: a village made with commands (beds, a bell and villagers who claim them), Bad Omen
// turning into Raid Omen only in the village, the raid starting when Raid Omen runs out, the raid
// bar, the raiders of each wave on Normal, the wave cooldown, victory and Hero of the Village.
const { make } = require('./survival-lib')
const h = make('Raider', 1200000)
const { bot, Vec3, sleep } = h
const X = 7000; const Y = 200; const Z = 0
const R = 14

const flat = (j) => {
  if (j === null || j === undefined) return ''
  if (typeof j !== 'object') return String(j)
  if (Object.keys(j).length === 1 && '' in j) return flat(j[''])
  if (j.translate) return j.translate + (j.with ? '(' + j.with.map(flat).join(',') + ')' : '')
  return (j.text ?? '') + (j.extra ? j.extra.map(flat).join('') : '')
}
const plain = (t) => {
  if (t === null || typeof t !== 'object') return t
  if (t.type === 'compound') return Object.fromEntries(Object.entries(t.value).map(([k, v]) => [k, plain(v)]))
  if (t.type === 'list') return (t.value.value || []).map(v => plain({ type: t.value.type, value: v }))
  if (t.type !== undefined && 'value' in t) return plain(t.value)
  return t
}
// Raid bar events: add/remove and every title it shows.
const bar = []
bot._client.on('boss_bar', (p) => {
  const title = p.title !== undefined ? flat(plain(p.title)) : null
  if (p.action === 0) bar.push(`add:${title}`)
  else if (p.action === 1) bar.push('remove')
  else if (p.action === 3 && bar[bar.length - 1] !== title) bar.push(title)
})
// Advancements the bot is told about, such as Voluntary Exile for killing a raid captain.
const advancements = []
bot._client.on('system_chat', (p) => {
  const text = flat(plain(p.content))
  if (text.startsWith('chat.type.advancement')) advancements.push(text)
})
const takeBar = () => { const s = bar.join('>') || 'none'; bar.length = 0; return s }
const effectNames = () => Object.keys(bot.entity.effects || {}).map(id => (bot.registry.effects[id] || {}).name).filter(Boolean).map(n => n.replace(/([a-z])([A-Z])/g, '$1_$2').toLowerCase()).sort().join(',') || 'none'
const hasEffect = (name) => effectNames().split(',').includes(name)
const RAIDERS = ['pillager', 'vindicator', 'evoker', 'witch', 'ravager']
const raiders = () => Object.values(bot.entities).filter(e => RAIDERS.includes(e.name) && e.position.distanceTo(new Vec3(X, Y, Z)) < 100)
const census = () => {
  const c = {}
  for (const e of raiders()) c[e.name] = (c[e.name] || 0) + 1
  return RAIDERS.filter(n => c[n]).map(n => `${n}x${c[n]}`).join(',') || 'none'
}

h.start(async () => {
  h.cmd('difficulty normal')
  await h.arena(X, Y, Z, R)
  h.cmd('time set 1000')
  h.cmd('weather clear')
  h.cmd(`fill ${X - R} ${Y + 1} ${Z - R} ${X + R} ${Y + 3} ${Z - R} glass`)
  h.cmd(`fill ${X - R} ${Y + 1} ${Z + R} ${X + R} ${Y + 3} ${Z + R} glass`)
  h.cmd(`fill ${X - R} ${Y + 1} ${Z - R} ${X - R} ${Y + 3} ${Z + R} glass`)
  h.cmd(`fill ${X + R} ${Y + 1} ${Z - R} ${X + R} ${Y + 3} ${Z + R} glass`)
  h.cmd(`setblock ${X} ${Y + 1} ${Z + 6} bell[attachment=floor]`)
  for (const dx of [-6, -2, 2, 6]) {
    h.cmd(`setblock ${X + dx} ${Y + 1} ${Z - 6} white_bed[part=head,facing=north]`)
    h.cmd(`setblock ${X + dx} ${Y + 1} ${Z - 5} white_bed[part=foot,facing=north]`)
    h.cmd(`summon villager ${X + dx + 0.5} ${Y + 1} ${Z + 0.5} {Invulnerable:1b}`)
  }
  await h.ticks(20)

  // Outside the village Bad Omen stays Bad Omen.
  h.cmd(`execute in minecraft:overworld run tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5}`)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.ticks(10)
  h.cmd(`effect give ${bot.username} bad_omen 6000 0`)
  await h.ticks(20)
  h.result('omen_before_villagers_settle', effectNames())

  // Villagers claim the beds and the bell; poll until Bad Omen turns into Raid Omen.
  let settled = -1
  for (let s = 0; s < 120; s += 2) {
    if (hasEffect('raid_omen')) { settled = s; break }
    await sleep(2000)
  }
  h.result('village_recognised', settled >= 0)
  h.result('omen_in_village', effectNames())
  takeBar()

  // Raid Omen lasts 30 s; the raid starts when it runs out.
  const omenEnd = Date.now() + 40000
  while (Date.now() < omenEnd && hasEffect('raid_omen')) await sleep(500)
  await h.waitFor(() => bar.length > 0, 10000)
  h.result('raid_started_bar', bar[0] || 'none')

  for (let wave = 1; wave <= 7; wave++) {
    const spawned = await h.waitFor(() => raiders().length > 0, 40000)
    if (!spawned) { h.result(`wave${wave}`, 'none'); break }
    await h.ticks(20)
    h.result(`wave${wave}`, census())
    const leader = raiders().find(e => (e.equipment || [])[5] && e.equipment[5].name === 'white_banner')
    h.result(`wave${wave}_leader_banner`, Boolean(leader))
    const near = raiders().map(e => Math.round(Math.hypot(e.position.x - X, e.position.z - Z)))
    h.result(`wave${wave}_within_16`, near.every(d => d <= 16))
    // The player kills one raider (the captain in the first wave), the rest are killed by command.
    const captain = wave === 1 ? ',nbt={equipment:{head:{id:"minecraft:white_banner"}}}' : ''
    advancements.length = 0
    h.cmd(`damage @e[type=#minecraft:raiders,limit=1,sort=nearest,x=${X},y=${Y},z=${Z}${captain}] 1000 minecraft:player_attack by ${bot.username}`)
    await h.ticks(10)
    if (wave === 1) h.result('wave1_captain_kill_advancements', advancements.join('|') || 'none')
    h.cmd(`kill @e[type=#minecraft:raiders,x=${X},y=${Y},z=${Z},distance=..200]`)
    await h.waitFor(() => raiders().length === 0, 10000)
    await h.ticks(60)
    h.result(`wave${wave}_bar`, takeBar())
  }
  await h.waitFor(() => hasEffect('hero_of_the_village'), 20000)
  h.result('hero', hasEffect('hero_of_the_village'))
  await sleep(5000)
  h.result('victory_bar', takeBar())
  h.cmd('save-all')
  await h.ticks(20)
})
