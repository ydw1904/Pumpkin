// A skeleton shoots the survival bot with an enchanted bow. Vanilla applies the bow's Power to the
// arrow's damage and its Punch to the knockback (`AbstractArrow.onHitEntity`, `doKnockback`).
const { make } = require('./survival-lib')
const h = make('Target')
const { bot } = h
const X = 9600; const Y = 200; const Z = 0

const velocities = []
bot._client.on('entity_velocity', (p) => {
  if (p.entityId === bot.entity.id) velocities.push({ t: Date.now(), kb: Math.hypot(p.velocity.x, p.velocity.z) })
})
let firstHit = null
bot.on('health', () => {
  if (firstHit && firstHit.damage === null && bot.health < firstHit.before) {
    firstHit.damage = firstHit.before - bot.health
    firstHit.t = Date.now()
  }
})

h.start(async () => {
  h.cmd('difficulty normal')
  await h.arena(X, Y, Z, 12)
  for (const [label, bow] of [['plain', 'bow'], ['punch2', 'bow[enchantments={punch:2}]'], ['power5', 'bow[enchantments={power:5}]']]) {
    h.cmd('kill @e[type=!player]')
    h.cmd(`effect give ${bot.username} instant_health 1 10`)
    await h.waitFor(() => bot.health >= 20, 5000)
    h.cmd(`tp ${bot.username} ${X + 0.5} ${Y + 1} ${Z + 0.5}`)
    await h.ticks(20)
    firstHit = { before: bot.health, damage: null, t: 0 }
    h.cmd(`summon skeleton ${X + 0.5} ${Y + 1} ${Z + 9.5} {PersistenceRequired:1b}`)
    h.cmd(`item replace entity @e[type=skeleton,limit=1] weapon.mainhand with ${bow}`)
    h.cmd(`item replace entity @e[type=skeleton,limit=1] armor.head with leather_helmet`)
    const hit = await h.waitFor(() => firstHit.damage !== null, 15000)
    h.cmd('kill @e[type=skeleton]')
    await h.ticks(10)
    // The strongest push the bot was sent around the hit: hurt knockback alone is about 0.4.
    const kb = Math.max(0, ...velocities.filter((v) => Math.abs(v.t - firstHit.t) < 1000).map((v) => v.kb))
    // The damage has a random part (`setBaseDamageFromMob`), so only the range is compared.
    h.result(`${label}.damage`, hit ? (firstHit.damage >= 7 ? 'high' : 'low') : 'no_hit')
    h.result(`${label}.knockback`, !hit ? 'none' : kb > 1.0 ? 'strong' : 'normal')
    firstHit = null
    await h.ticks(20)
  }
  // A summoned arrow carries its bow in the `weapon` tag, and vanilla reads Power and Punch from
  // it when the arrow hits.
  h.cmd(`gamemode creative ${bot.username}`)
  for (const [label, enchantment] of [['weapon_plain', ''], ['weapon_punch2', '"minecraft:punch":2'], ['weapon_power5', '"minecraft:power":5']]) {
    h.cmd('kill @e[type=!player]')
    // The last husk lingers in its death animation; wait until it is gone.
    await h.waitFor(() => !Object.values(bot.entities).some((e) => e.name === 'husk'), 5000)
    h.cmd(`summon husk ${X + 5.5} ${Y + 1} ${Z + 6.5} {NoAI:1b,Silent:1b,PersistenceRequired:1b}`)
    await h.waitFor(() => Object.values(bot.entities).some((e) => e.name === 'husk'), 5000)
    await h.ticks(10)
    const husk = Object.values(bot.entities).find((e) => e.name === 'husk')
    let push = 0
    const onVelocity = (p) => { if (husk && p.entityId === husk.id) push = Math.max(push, Math.hypot(p.velocity.x, p.velocity.z)) }
    bot._client.on('entity_velocity', onVelocity)
    const weapon = `weapon:{id:"minecraft:bow",count:1${enchantment ? `,components:{"minecraft:enchantments":{${enchantment}}}` : ''}}`
    h.cmd(`summon arrow ${X + 5.5} ${Y + 1.8} ${Z + 3.5} {Motion:[0.0,0.0,1.5],${weapon}}`)
    await h.ticks(30)
    bot._client.removeListener('entity_velocity', onVelocity)
    const health = husk && husk.metadata[9]
    h.result(`${label}.husk_health`, typeof health === 'number' ? Math.round(health) : 'unknown')
    h.result(`${label}.knockback`, push > 1.0 ? 'strong' : push > 0 ? 'normal' : 'none')
  }
})
