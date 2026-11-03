// Block commands and their side effects: /setblock with block entity NBT, neighbour shape
// updates from /setblock, /fill and falling blocks, observers seeing /fill, /fill counts,
// /time queries and forceloaded chunks accepting /fill.
// pumpkin-settings: allow_end=true allow_nether=true
const { make } = require('./survival-lib')
const h = make('Builder')
const { bot } = h
const X = 6400; const Y = 200; const Z = 0

const flat = (j) => {
  if (j === null || j === undefined) return ''
  if (typeof j !== 'object') return String(j)
  // A long as [high, low], or a component written as {"": text}.
  if (Array.isArray(j) && j.length === 2 && j.every(Number.isInteger)) return String(BigInt(j[0]) * 4294967296n + BigInt(j[1] >>> 0))
  if (Object.keys(j).length === 1 && '' in j) return flat(j[''])
  if (j.translate) return j.translate + (j.with ? '(' + j.with.map(flat).join(',') + ')' : '')
  if (j.text === undefined && !j.extra) return JSON.stringify(j)
  return (j.text ?? '') + (j.extra ? j.extra.map(flat).join('') : '')
}
const msgs = []
// Raw NBT text components, because prismarine-chat drops number arguments.
const plain = (t) => {
  if (t === null || typeof t !== 'object') return t
  if (t.type === 'compound') return Object.fromEntries(Object.entries(t.value).map(([k, v]) => [k, plain(v)]))
  if (t.type === 'list') return (t.value.value || []).map(v => plain({ type: t.value.type, value: v }))
  if (t.type !== undefined && 'value' in t) return plain(t.value)
  return t
}
bot._client.on('system_chat', (p) => { if (!p.isActionBar) msgs.push(flat(plain(p.content))) })
// Runs a command as the bot and returns the feedback it got.
async function run (c, wait = 10) {
  msgs.length = 0
  bot.chat('/' + c)
  await h.waitFor(() => msgs.length > 0, 3000)
  await h.ticks(wait)
  return msgs.join('|') || 'none'
}
const props = (x, y, z) => { const b = h.block(x, y, z); return b ? b.getProperties() : {} }

h.start(async () => {
  await h.arena(X, Y, Z, 10)
  h.cmd(`gamemode creative ${bot.username}`)
  h.cmd('time set 6000')
  await h.ticks(10)
  const y = Y + 1

  // /setblock with block entity NBT, read back through a comparator and /execute if data.
  h.result('setblock_nbt', await run(`setblock ${X - 8} ${y} ${Z - 8} chest[facing=south]{Items:[{Slot:0b,id:"minecraft:stone",count:1}]}`))
  h.cmd(`setblock ${X - 8} ${y} ${Z - 7} comparator[facing=north]`)
  h.cmd(`setblock ${X - 8} ${y} ${Z - 6} redstone_wire`)
  await h.ticks(10)
  h.result('setblock_nbt_block', h.blockName(X - 8, y, Z - 8))
  h.result('setblock_nbt_comparator', props(X - 8, y, Z - 6).power)
  h.result('setblock_nbt_data', await run(`execute if data block ${X - 8} ${y} ${Z - 8} Items[{id:"minecraft:stone"}]`))
  h.result('setblock_nbt_same', await run(`setblock ${X - 8} ${y} ${Z - 8} chest[facing=south]{Items:[{Slot:0b,id:"minecraft:stone",count:1}]}`))
  h.result('setblock_nbt_change', await run(`setblock ${X - 8} ${y} ${Z - 8} chest[facing=south]{Items:[{Slot:0b,id:"minecraft:dirt",count:2}]}`))
  h.result('setblock_same_state', await run(`setblock ${X - 8} ${y} ${Z - 7} comparator[facing=north]`))
  h.result('fill_nbt', await run(`fill ${X - 6} ${y} ${Z - 8} ${X - 5} ${y} ${Z - 8} barrel{Items:[{Slot:3b,id:"minecraft:apple",count:5}]}`))
  h.result('fill_nbt_data', await run(`execute if data block ${X - 5} ${y} ${Z - 8} Items[{Slot:3b,count:5}]`))
  h.result('setblock_bad_nbt', await run(`setblock ${X - 4} ${y} ${Z - 8} chest{Items:[}`))

  // Shapes: a note block takes the instrument of the block under it, fences connect.
  h.cmd(`setblock ${X - 2} ${y} ${Z - 8} gold_block`)
  await h.ticks(5)
  await run(`setblock ${X - 2} ${y + 1} ${Z - 8} note_block`)
  h.result('setblock_note_instrument', props(X - 2, y + 1, Z - 8).instrument)
  await run(`setblock ${X - 1} ${y + 1} ${Z - 8} note_block[instrument=flute]`)
  h.result('setblock_note_defined', props(X - 1, y + 1, Z - 8).instrument)
  h.cmd(`setblock ${X} ${y} ${Z - 8} oak_fence`)
  await h.ticks(5)
  await run(`setblock ${X + 1} ${y} ${Z - 8} oak_fence`)
  h.result('setblock_fence', `${props(X, y, Z - 8).east}/${props(X + 1, y, Z - 8).west}`)
  await run(`setblock ${X + 3} ${y} ${Z - 8} oak_fence strict`)
  h.result('setblock_fence_strict', `${props(X + 2, y, Z - 8).east}/${props(X + 3, y, Z - 8).west}`)
  await run(`fill ${X + 5} ${y} ${Z - 8} ${X + 8} ${y} ${Z - 8} oak_fence`)
  h.result('fill_fences', [5, 6, 7, 8].map(i => `${props(X + i, y, Z - 8).west}${props(X + i, y, Z - 8).east}`).join(','))
  h.cmd(`fill ${X + 5} ${y + 1} ${Z - 6} ${X + 8} ${y + 1} ${Z - 6} gold_block`)
  await h.ticks(5)
  await run(`fill ${X + 5} ${y + 2} ${Z - 6} ${X + 8} ${y + 2} ${Z - 6} note_block`)
  h.result('fill_note_instrument', props(X + 6, y + 2, Z - 6).instrument)

  // A note block on sand takes harp once the sand falls away.
  h.cmd(`setblock ${X - 8} ${y + 1} ${Z - 2} barrier`)
  h.cmd(`setblock ${X - 8} ${y + 2} ${Z - 2} sand`)
  await h.ticks(5)
  h.cmd(`setblock ${X - 8} ${y + 3} ${Z - 2} note_block`)
  await h.ticks(5)
  h.result('note_on_sand', props(X - 8, y + 3, Z - 2).instrument)
  h.cmd(`setblock ${X - 8} ${y + 1} ${Z - 2} air`)
  await h.ticks(40)
  h.result('note_after_sand_fell', `${h.blockName(X - 8, y + 2, Z - 2)} ${props(X - 8, y + 3, Z - 2).instrument}`)

  // Observers watching blocks changed by /fill and /setblock.
  const observerPulses = async (c) => {
    const seen = []
    const fn = (o, n) => { if (n && n.position.x === X - 4 && n.position.y === y && n.position.z === Z) seen.push(n.getProperties().powered) }
    bot.on('blockUpdate', fn)
    h.cmd(c)
    await h.ticks(20)
    bot.removeListener('blockUpdate', fn)
    return seen.join('>') || 'none'
  }
  h.cmd(`setblock ${X - 4} ${y} ${Z} observer[facing=east]`)
  await h.ticks(10)
  h.result('observer_fill', await observerPulses(`fill ${X - 3} ${y} ${Z} ${X - 2} ${y} ${Z} stone`))
  h.result('observer_fill_strict', await observerPulses(`fill ${X - 3} ${y} ${Z} ${X - 2} ${y} ${Z} dirt strict`))
  h.result('observer_setblock', await observerPulses(`setblock ${X - 3} ${y} ${Z} glass`))
  h.result('observer_setblock_strict', await observerPulses(`setblock ${X - 3} ${y} ${Z} stone strict`))

  // /fill counts only changed blocks.
  h.cmd(`fill ${X} ${y} ${Z + 2} ${X + 2} ${y} ${Z + 4} stone`)
  await h.ticks(5)
  h.result('fill_unchanged', await run(`fill ${X} ${y} ${Z + 2} ${X + 2} ${y} ${Z + 4} stone`))
  h.result('fill_partly', await run(`fill ${X} ${y} ${Z + 2} ${X + 2} ${y} ${Z + 5} stone`))
  h.result('fill_hollow', await run(`fill ${X} ${y + 1} ${Z + 2} ${X + 2} ${y + 3} ${Z + 4} glass hollow`))
  h.result('fill_outline', await run(`fill ${X} ${y + 1} ${Z + 2} ${X + 2} ${y + 3} ${Z + 4} stone outline`))
  h.result('fill_destroy', await run(`fill ${X} ${y + 1} ${Z + 2} ${X + 2} ${y + 3} ${Z + 4} air destroy`))
  h.result('fill_keep', await run(`fill ${X} ${y} ${Z + 2} ${X + 2} ${y + 1} ${Z + 4} dirt keep`))
  h.result('setblock_keep', await run(`setblock ${X} ${y} ${Z + 2} dirt keep`))

  // /execute store: results and success flags, failures, forks, nested runs and every target.
  await run('scoreboard objectives add st dummy')
  const score = async (holder) => (await run(`scoreboard players get ${holder} st`, 2)).split('|')[0]
  const stores = [
    ['result', 'execute store result score #a st run scoreboard players set #x st 7', '#a'],
    ['success', 'execute store success score #b st run scoreboard players set #x st 0', '#b'],
    ['result_fail', 'execute store result score #c st run kill @e[tag=parity_none]', '#c'],
    ['success_fail', 'execute store success score #d st run kill @e[tag=parity_none]', '#d'],
    ['nested_outer', 'execute store result score #n1 st run execute store result score #n2 st run scoreboard players set #x st 9', '#n1'],
    ['nested_inner', null, '#n2'],
    ['if_entity_none', 'execute store result score #f st if entity @e[tag=parity_none]', '#f'],
    ['unless_entity_none', 'execute store result score #g st unless entity @e[tag=parity_none]', '#g'],
    ['if_data_missing', 'execute store success score #h st if data storage parity:t missing', '#h']
  ]
  for (const [key, c, holder] of stores) {
    if (c) h.result(`store_${key}_msg`, await run(c, 2))
    h.result(`store_${key}`, await score(holder))
  }
  for (let i = 0; i < 3; i++) h.cmd(`summon armor_stand ${X + 4 + i}.5 ${y} ${Z + 6}.5 {Tags:["parity_st"],NoGravity:1b}`)
  await h.ticks(5)
  h.result('store_if_entity', (await run('execute store result score #i st if entity @e[tag=parity_st]', 2)) + ' ' + await score('#i'))
  await run('scoreboard players set #cnt st 0', 2)
  h.result('store_fork_before_as', (await run('execute store result score #e st as @e[tag=parity_st] run scoreboard players add #cnt st 1', 2)) + ' ' + await score('#e'))
  await run('execute as @e[tag=parity_st] store result score @s st run data get entity @s Pos[0] 10', 2)
  h.result('store_fork_after_as', (await run(`scoreboard players get @e[tag=parity_st,limit=1,sort=nearest,x=${X + 4},y=${y},z=${Z + 6}] st`, 2)).split('|')[0])
  await run('scoreboard players set #m st 5', 2)
  h.result('store_modifier_error', (await run('execute store success score #m st if score #q parity_missing matches 1 run say x', 2)) + ' ' + await score('#m'))
  h.result('store_storage', (await run('execute store result storage parity:t v int 2.5 run scoreboard players get #a st', 2)) + ' ' + await run('data get storage parity:t v', 2))
  await run('execute store result storage parity:t b byte 1 run scoreboard players set #y st 300', 2)
  await run('execute store result storage parity:t d double 0.1 run scoreboard players set #y st 3', 2)
  await run('execute store success storage parity:t s short 3 run scoreboard players set #y st 3', 2)
  for (const k of ['b', 'd', 's']) h.result(`store_storage_${k}`, await run(`data get storage parity:t ${k}`, 2))
  await run('bossbar add parity:b "b"', 2)
  await run('execute store result bossbar parity:b max run scoreboard players set #y st 50', 2)
  await run('execute store result bossbar parity:b value run scoreboard players set #y st 20', 2)
  h.result('store_bossbar', (await run('bossbar get parity:b max', 2)) + ' ' + await run('bossbar get parity:b value', 2))
  h.result('store_bossbar_unknown', await run('execute store result bossbar parity:none value run say x', 2))
  h.cmd(`setblock ${X + 4} ${y} ${Z + 8} barrel{Items:[{Slot:0b,id:"minecraft:apple",count:5}]}`)
  await h.ticks(5)
  await run(`execute store result block ${X + 4} ${y} ${Z + 8} Items[0].count byte 1 run scoreboard players set #y st 12`, 2)
  h.result('store_block', await run(`data get block ${X + 4} ${y} ${Z + 8} Items[0].count`, 2))
  await run(`data modify block ${X + 4} ${y} ${Z + 8} Items[0].count set value 7b`, 2)
  h.result('modify_block', await run(`data get block ${X + 4} ${y} ${Z + 8} Items[0].count`, 2))
  h.result('store_block_invalid', await run(`execute store result block ${X + 5} ${y} ${Z + 8} Items[0].count byte 1 run say x`, 2))
  await run(`execute store result entity @e[tag=parity_st,limit=1,sort=nearest,x=${X + 4},y=${y},z=${Z + 6}] Rotation[0] float 0.5 run scoreboard players set #y st 90`, 2)
  h.result('store_entity', await run(`data get entity @e[tag=parity_st,limit=1,sort=nearest,x=${X + 4},y=${y},z=${Z + 6}] Rotation[0]`, 2))
  h.result('store_player', await run(`execute store result entity ${bot.username} Health float 1 run scoreboard players set #y st 3`, 2))
  h.cmd('kill @e[tag=parity_st]')
  h.cmd('bossbar remove parity:b')
  await h.ticks(10)

  // /execute if|unless feedback and counts.
  for (const [key, c] of [
    ['if_data_count', 'execute if data storage parity:t b'],
    ['unless_data_missing', 'execute unless data storage parity:t nope'],
    ['unless_data_present', 'execute unless data storage parity:t b'],
    ['if_blocks_too_big', `execute if blocks ${X} ${y} ${Z} ${X + 40} ${y + 40} ${Z + 40} ${X} ${y} ${Z} all`],
    ['if_blocks_masked', `execute if blocks ${X} ${y + 5} ${Z} ${X + 2} ${y + 6} ${Z + 2} ${X} ${y + 5} ${Z} masked`],
    ['if_blocks_chest', `execute if blocks ${X - 8} ${y} ${Z - 8} ${X - 8} ${y} ${Z - 8} ${X - 6} ${y} ${Z - 8} all`],
    ['if_block_unloaded', 'execute if block 1000000 100 1000000 stone'],
    ['if_predicate_unknown', 'execute if predicate parity:none'],
    ['store_unless_data', 'execute store result score #u st unless data storage parity:t b']
  ]) h.result(`cond_${key}`, await run(c, 2))

  // /time with the clock paused, so the values are the same on both servers.
  await run('time pause')
  await run('time set 30000')
  for (const [i, q] of ['query day', 'query minecraft:day repetition', 'query moon', 'query moon repetition', 'query early_game', 'query villager_schedule', 'query time', 'set day', 'query day', 'query day repetition', 'set day', 'set minecraft:wake_up_from_sleep', 'set midnight', 'query day', 'set 5', 'set 5', 'add -40000', 'query time'].entries()) {
    h.result(`time_${i}_${q.replace(/[ :]/g, '_')}`, (await run('time ' + q, 2)).split('|')[0])
  }
  for (const q of ['query daytime', 'of minecraft:the_end query day', 'set -1', 'set foo', 'pause', 'resume', 'resume', 'rate 1', 'rate 2', 'query gametime']) {
    h.result(`time_${q.replace(/[ :]/g, '_')}`, (await run('time ' + q, 2)).replace(/\(.*/, ''))
  }
  // Each dimension's default clock: none in the Nether, its own in the End.
  h.result('time_nether_query', await run('execute in minecraft:the_nether run time query time', 2))
  h.result('time_nether_gametime', (await run('execute in minecraft:the_nether run time query gametime', 2)).replace(/\(.*/, ''))
  h.result('time_end_query', (await run('execute in minecraft:the_end run time query time', 2)).replace(/,\d+\)$/, ')'))
  h.result('time_nether_of_overworld', (await run('execute in minecraft:the_nether run time of minecraft:overworld query time', 2)).replace(/,\d+\)$/, ')'))
  await run('time rate 1')
  await run('time set 6000')

  // Forceloaded chunks: /fill works across a forceloaded 4x4 chunk area once it loads. The time
  // is logged; a debug build of Pumpkin takes a few seconds where a release build is as fast as
  // vanilla.
  const forced = async (dim, x, z) => {
    const t0 = Date.now()
    await run(`execute in ${dim} run forceload add ${x} ${z} ${x + 63} ${z + 63}`, 0)
    while (Date.now() - t0 < 30000) {
      const r = await run(`execute in ${dim} run fill ${x} 100 ${z} ${x + 63} 100 ${z + 63} stone`, 0)
      if (r.includes('commands.fill.success')) {
        const ms = Date.now() - t0
        console.log(`forceload ${dim}: ${ms} ms`)
        await run(`execute in ${dim} run forceload remove ${x} ${z} ${x + 63} ${z + 63}`, 0)
        return 'loaded'
      }
      await h.ticks(1)
    }
    return 'never'
  }
  h.result('forceload_overworld', await forced('minecraft:overworld', 30000, 30000))
  h.result('forceload_end', await forced('minecraft:the_end', 3000, 3000))
  h.result('forceload_nether', await forced('minecraft:the_nether', 3000, 3000))
})
