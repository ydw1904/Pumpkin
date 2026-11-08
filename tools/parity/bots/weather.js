// Weather and time: /weather and /time feedback and what the client sees, lightning converting
// mobs (charged creeper, zombified piglin, witch, mooshroom colour), a channeling trident in a
// thunderstorm, and snow layers and ice forming in a cold biome while it snows.
const { make } = require('./survival-lib')
const h = make('Weatherman')
const { bot } = h
const nbt = require('prismarine-nbt')
const X = 6400; const Y = 200; const Z = 0

// Reads the raw component: prismarine-chat drops number arguments, which vanilla sends as NBT
// values (wrapped in a compound under the '' key when the list mixes types). Failures arrive as
// an empty red text with the translation in `extra`.
const value = (v) => Array.isArray(v) && v.length === 2 && v.every(n => typeof n === 'number')
  ? String((BigInt(v[0]) << 32n) | BigInt(v[1] >>> 0))
  : v
const arg = (w) => typeof w === 'object' && !Array.isArray(w) ? (w.text ?? w.translate ?? value(w[''])) : value(w)
const describe = (j) => typeof j !== 'object' || j === null
  ? null
  : j.translate
    ? j.translate + (j.with ? '(' + j.with.map(arg).join(',') + ')' : '')
    : (j.extra ?? []).map(describe).find(s => s) ?? null
const msgs = []
bot._client.on('system_chat', (p) => {
  if (p.isActionBar) return
  const j = p.content && p.content.type ? nbt.simplify(p.content) : typeof p.content === 'string' ? JSON.parse(p.content) : p.content
  const s = describe(j)
  if (s) msgs.push(s)
})
const take = () => { const s = msgs.join('|') || 'none'; msgs.length = 0; return s }
const say = async (c) => { take(); bot.chat(c); await h.ticks(10); return take() }
const mobs = (name, r = 24) => Object.values(bot.entities).filter(e => e.name === name && e.position.distanceTo(bot.entity.position) < r)

h.start(async () => {
  await h.arena(X, Y, Z, 12)
  h.cmd('gamerule advance_weather false')
  await h.ticks(5)
  h.result('weather_rain', await say('/weather rain'))
  await h.ticks(150)
  h.result('client_raining', `${bot.isRaining} rain=${bot.rainState.toFixed(2)}`)
  h.result('weather_thunder', await say('/weather thunder 600'))
  await h.ticks(150)
  h.result('client_thunder', bot.thunderState.toFixed(2))
  h.result('weather_clear', await say('/weather clear'))
  await h.ticks(150)
  h.result('client_clear', `${bot.isRaining} rain=${bot.rainState.toFixed(2)}`)
  h.result('weather_bad', await say('/weather snow'))

  // A stopped clock keeps the queried times from depending on when each command lands.
  h.cmd('gamerule advance_time false')
  await h.ticks(5)
  h.result('time_set_day', await say('/time set day'))
  h.result('time_set_night', await say('/time set night'))
  h.result('time_set_noon', await say('/time set noon'))
  h.result('time_set_midnight', await say('/time set midnight'))
  h.result('time_add', await say('/time add 100'))
  h.result('time_query_daytime', await say('/time query daytime'))
  h.result('time_query_day', await say('/time query day'))
  h.result('time_set_1d', await say('/time set 1d'))
  h.result('time_set_negative', await say('/time set -5'))
  h.cmd('time set 6000')
  await h.ticks(10)
  h.result('client_time', `${bot.time.timeOfDay} cycle=${bot.time.doDaylightCycle}`)

  // Lightning conversions.
  h.cmd(`summon creeper ${X + 5.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  h.cmd(`summon pig ${X - 5.5} ${Y + 1} ${Z + 0.5} {NoAI:1b}`)
  h.cmd(`summon villager ${X + 0.5} ${Y + 1} ${Z + 5.5} {NoAI:1b}`)
  h.cmd(`summon mooshroom ${X + 0.5} ${Y + 1} ${Z - 5.5} {NoAI:1b,Type:"red"}`)
  h.cmd('difficulty normal')
  await h.ticks(20)
  for (const [x, z] of [[5.5, 0.5], [-5.5, 0.5], [0.5, 5.5], [0.5, -5.5]]) h.cmd(`summon lightning_bolt ${X + x} ${Y + 1} ${Z + z}`)
  await h.ticks(40)
  const creeper = mobs('creeper')[0]
  h.result('lightning_creeper_charged', creeper ? creeper.metadata[17] : 'gone')
  h.result('lightning_pig', `pigs=${mobs('pig').length} zombified=${mobs('zombified_piglin').length}`)
  h.result('lightning_villager', `villagers=${mobs('villager').length} witches=${mobs('witch').length}`)
  const moo = mobs('mooshroom')[0]
  h.result('lightning_mooshroom', moo ? moo.metadata[18] : 'gone')
  h.result('lightning_fire', bot.findBlocks({ matching: (b) => b && b.name === 'fire', maxDistance: 12, count: 50 }).length > 0)
  h.cmd('kill @e[type=!player]')
  h.cmd('fill ~-12 ~ ~-12 ~12 ~5 ~12 air replace fire')
  await h.ticks(10)

  // A channeling trident hitting a mob under open sky in a thunderstorm calls lightning.
  h.cmd('weather thunder 600')
  h.cmd(`summon creeper ${X + 3.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,PersistenceRequired:1b}`)
  await h.ticks(20)
  h.cmd(`summon trident ${X + 3.5} ${Y + 4} ${Z + 0.5} {Motion:[0.0,-2.0,0.0],item:{id:"minecraft:trident",count:1,components:{"minecraft:enchantments":{"minecraft:channeling":1}}}}`)
  await h.ticks(40)
  const c2 = mobs('creeper')[0]
  h.result('channeling_creeper_charged', c2 ? c2.metadata[17] : 'gone')
  h.cmd('kill @e[type=!player]')
  h.cmd('weather clear')

  // Snow and ice: a snowy biome under snowfall with fast precipitation ticks.
  h.cmd(`fillbiome ${X - 12} ${Y - 16} ${Z - 12} ${X + 12} ${Y + 16} ${Z + 12} minecraft:snowy_plains`)
  h.cmd(`fill ${X - 12} ${Y} ${Z - 12} ${X + 12} ${Y} ${Z + 12} stone`)
  h.cmd(`fill ${X + 4} ${Y} ${Z + 4} ${X + 8} ${Y} ${Z + 8} water`)
  h.cmd(`fill ${X - 8} ${Y} ${Z + 4} ${X - 4} ${Y} ${Z + 8} water`)
  h.cmd(`setblock ${X - 6} ${Y + 1} ${Z + 6} glowstone`)
  h.cmd('gamerule max_snow_accumulation_height 3')
  h.cmd('weather rain')
  h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z - 6.5}`)
  await h.ticks(20)
  h.cmd('gamerule random_tick_speed 1000')
  await h.ticks(1200)
  h.cmd('gamerule random_tick_speed 3')
  h.cmd('weather clear')
  await h.ticks(20)
  const layers = {}
  for (let x = X - 3; x <= X + 3; x++) for (let z = Z - 3; z <= Z + 3; z++) { const b = h.block(x, Y + 1, z); const k = b.name === 'snow' ? 'snow' + b.getProperties().layers : b.name; layers[k] = (layers[k] || 0) + 1 }
  h.result('snow_layers', Object.keys(layers).sort().map(k => `${k}:${layers[k] > 0 ? 'y' : ''}`).join(','))
  h.result('snow_max_layers_3', Object.keys(layers).every(k => !k.startsWith('snow') || Number(k.slice(4)) <= 3))
  const ice = []
  for (let x = X + 4; x <= X + 8; x++) for (let z = Z + 4; z <= Z + 8; z++) ice.push(h.blockName(x, Y, z) === 'ice' ? 'I' : 'w')
  h.result('ice_pool_all_frozen', ice.every(c => c === 'I'))
  const lit = []
  for (let x = X - 8; x <= X - 4; x++) for (let z = Z + 4; z <= Z + 8; z++) lit.push(h.blockName(x, Y, z) === 'ice' ? 'I' : 'w')
  h.result('ice_lit_pool', lit.join(''))
  h.cmd('gamerule advance_weather true')
  h.cmd('gamerule advance_time true')
})
