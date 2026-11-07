// Functions and what they return: /return, return run, return fail, macros, function tags,
// execute if function, the fork limit, execute if items and data command edge cases. The check
// writes its data pack into the world and enables it with /reload (vanilla) or /datapack enable.
const fs = require('fs')
const path = require('path')
const { make } = require('./survival-lib')
const h = make('Functions')
const { bot } = h
const X = 6600; const Y = 200; const Z = 0

const flat = (j) => {
  if (j === null || j === undefined) return ''
  if (typeof j !== 'object') return String(j)
  if (Array.isArray(j) && j.length === 2 && j.every(Number.isInteger)) return String(BigInt(j[0]) * 4294967296n + BigInt(j[1] >>> 0))
  if (Object.keys(j).length === 1 && '' in j) return flat(j[''])
  if (j.translate) return j.translate + (j.with ? '(' + j.with.map(flat).join(',') + ')' : '')
  if (j.text === undefined && !j.extra) return JSON.stringify(j)
  return (j.text ?? '') + (j.extra ? j.extra.map(flat).join('') : '')
}
const msgs = []
const plain = (t) => {
  if (t === null || typeof t !== 'object') return t
  if (t.type === 'compound') return Object.fromEntries(Object.entries(t.value).map(([k, v]) => [k, plain(v)]))
  if (t.type === 'list') return (t.value.value || []).map(v => plain({ type: t.value.type, value: v }))
  if (t.type !== undefined && 'value' in t) return plain(t.value)
  return t
}
bot._client.on('system_chat', (p) => { if (!p.isActionBar) msgs.push(flat(plain(p.content))) })
async function run (c, wait = 2, timeout = 3000) {
  msgs.length = 0
  bot.chat('/' + c)
  await h.waitFor(() => msgs.length > 0, timeout)
  await h.ticks(wait)
  return msgs.join('|') || 'none'
}
const score = async (holder) => (await run(`scoreboard players get ${holder} fn`)).split('|')[0]

const FUNCTIONS = {
  ret5: 'return 5',
  ret_after: 'scoreboard players set #a fn 1\nreturn 7\nscoreboard players set #a fn 2',
  noret: 'scoreboard players add #b fn 1',
  fail: 'return fail',
  retrun: 'return run scoreboard players set #c fn 9\nscoreboard players set #c fn 10',
  retrun_none: 'return run execute if entity @e[tag=parity_none] run say x',
  retrun_fn: 'return run function p:ret5\nscoreboard players set #d fn 1',
  retrun_fn_noret: 'return run function p:noret',
  retrun_error: 'return run data get storage p:none missing',
  macro: '$scoreboard players set #m fn $(v)\n$return $(v)',
  macro_text: '$data modify storage p:out t set value "$(t)"',
  loop: 'scoreboard players add #l fn 1\nexecute if score #l fn matches ..9 run function p:loop',
  count_ret: 'scoreboard players add #cr fn 1\nreturn 1',
  fork_ret: 'execute as @e[tag=parity_fr] run return run scoreboard players add #fr fn 1\nscoreboard players set #fr fn 100',
  feedback: 'data get storage p:out t\nscoreboard players set #fb fn 1',
  continued: 'scoreboard players \\\n  set #cont fn 3',
  // `execute if function` nests 1500 deep; the rest of each command runs after the call.
  deep: 'scoreboard players add #dp fn 1\nexecute if score #dp fn matches ..1499 unless function p:deep run scoreboard players add #dpr fn 1',
  // Never ends: stops at max_command_sequence_length.
  endless: 'scoreboard players add #inf fn 1\nexecute unless function p:endless run scoreboard players add #infr fn 1'
}

h.start(async () => {
  const pack = path.join(path.dirname(process.env.CMDS), 'world', 'datapacks', 'parity_fn')
  const fnDir = path.join(pack, 'data', 'p', 'function')
  fs.mkdirSync(fnDir, { recursive: true })
  fs.mkdirSync(path.join(pack, 'data', 'p', 'tags', 'function'), { recursive: true })
  fs.writeFileSync(path.join(pack, 'pack.mcmeta'), JSON.stringify({ pack: { description: 'parity functions', min_format: 121, max_format: 121 } }))
  for (const [name, body] of Object.entries(FUNCTIONS)) fs.writeFileSync(path.join(fnDir, `${name}.mcfunction`), body + '\n')
  fs.writeFileSync(path.join(pack, 'data', 'p', 'tags', 'function', 'both.json'), JSON.stringify({ values: ['p:ret5', 'p:ret_after'] }))

  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.ticks(10)
  await run('reload', 40)
  await run('datapack enable "file/parity_fn"', 40)
  await run('scoreboard objectives add fn dummy')

  h.result('fn_ret5', await run('function p:ret5'))
  h.result('fn_store_ret5', (await run('execute store result score #r fn run function p:ret5')) + ' ' + await score('#r'))
  await run('scoreboard players set #r2 fn 42')
  h.result('fn_store_noret', (await run('execute store result score #r2 fn run function p:noret')) + ' ' + await score('#r2'))
  await run('scoreboard players set #s fn 42')
  h.result('fn_store_fail', (await run('execute store success score #s fn run function p:fail')) + ' ' + await score('#s'))
  h.result('fn_ret_after', (await run('function p:ret_after')) + ' ' + await score('#a'))
  for (const name of ['retrun', 'retrun_none', 'retrun_fn', 'retrun_fn_noret', 'retrun_error']) {
    await run('scoreboard players set #x fn 42')
    await run('scoreboard players set #y fn 42')
    h.result(`fn_${name}`, await run(`function p:${name}`))
    await run(`execute store result score #x fn run function p:${name}`)
    await run(`execute store success score #y fn run function p:${name}`)
    h.result(`fn_${name}_store`, `${await score('#x')} ${await score('#y')}`)
  }
  h.result('fn_retrun_after', `${await score('#c')} ${await score('#d')}`)

  h.result('fn_macro', (await run('function p:macro {v:12}')) + ' ' + await score('#m'))
  h.result('fn_macro_missing', await run('function p:macro'))
  h.result('fn_macro_missing_one', await run('function p:macro {w:1}'))
  await run('data merge storage p:m {v:3,n:{v:4},s:"x",t:1.5f,l:[1,2]}')
  h.result('fn_macro_with', (await run('function p:macro with storage p:m')) + ' ' + await score('#m'))
  h.result('fn_macro_with_path', (await run('function p:macro with storage p:m n')) + ' ' + await score('#m'))
  h.result('fn_macro_with_missing', await run('function p:macro with storage p:m nope'))
  h.result('fn_macro_with_not_compound', await run('function p:macro with storage p:m s'))
  for (const v of ['1.5f', '0.1f', '2.0d', '1b', '"a b"', '[1,2]', '{a:1}', '1e20d', '-0.0d']) {
    await run(`function p:macro_text {t:${v}}`)
    h.result(`fn_macro_text_${v}`, await run('data get storage p:out t'))
  }

  h.result('fn_tag', await run('function #p:both'))
  h.result('fn_tag_store', (await run('execute store result score #t fn run function #p:both')) + ' ' + await score('#t'))
  h.result('fn_unknown', await run('function p:nope'))
  h.result('fn_loop', (await run('function p:loop')) + ' ' + await score('#l'))
  h.result('fn_feedback', (await run('function p:feedback')) + ' ' + await score('#fb'))
  h.result('fn_continued', (await run('function p:continued')) + ' ' + await score('#cont'))

  // Top-level /return and frames ending forks.
  h.result('return_top', await run('return 3'))
  h.result('return_top_store', (await run('execute store result score #tr fn run return 3')) + ' ' + await score('#tr'))
  h.result('return_fail_top', (await run('execute store success score #tf fn run return fail')) + ' ' + await score('#tf'))
  for (let i = 0; i < 3; i++) h.cmd(`summon armor_stand ${X + i}.5 ${Y + 1} ${Z + 3}.5 {Tags:["parity_fr"],NoGravity:1b}`)
  await h.ticks(5)
  h.result('fork_function_frames', (await run('execute as @e[tag=parity_fr] run function p:count_ret')) + ' ' + await score('#cr'))
  h.result('fork_return_in_function', (await run('function p:fork_ret')) + ' ' + await score('#fr'))
  h.result('fork_return_top', (await run('execute as @e[tag=parity_fr] run return run scoreboard players add #ft fn 1')) + ' ' + await score('#ft'))

  // execute if|unless function.
  for (const [key, c] of [
    ['if_ret5', 'execute if function p:ret5 run scoreboard players set #if1 fn 1'],
    ['if_noret', 'execute if function p:noret run scoreboard players set #if2 fn 1'],
    ['unless_noret', 'execute unless function p:noret run scoreboard players set #if3 fn 1'],
    ['if_fail', 'execute if function p:fail run scoreboard players set #if4 fn 1'],
    ['if_tag', 'execute if function #p:both run scoreboard players set #if5 fn 1'],
    ['if_unknown', 'execute if function p:nope run say x'],
    ['if_macro', 'execute if function p:macro run say x']
  ]) h.result(`cond_fn_${key}`, await run(c))
  h.result('cond_fn_deep', (await run('function p:deep')) + ' ' + await score('#dp') + ' ' + await score('#dpr'))
  h.result('cond_fn_endless', (await run('function p:endless', 10, 60000)) + ' ' + await score('#inf') + ' ' + await score('#infr'))
  h.result('cond_fn_scores',(await Promise.all(['#if1', '#if2', '#if3', '#if4', '#if5'].map(async (k) => (await score(k)).replace(/.*\((#if\d),(\d+).*/, '$1=$2')))).join(' '))

  // The fork limit.
  await run('gamerule max_command_forks 2')
  h.result('fork_limit', (await run('execute as @e[tag=parity_fr] run scoreboard players add #fl fn 1')) + ' ' + await score('#fl'))
  h.result('fork_limit_store', (await run('execute store result score #fl2 fn as @e[tag=parity_fr] run scoreboard players add #fl fn 1')) + ' ' + await score('#fl2'))
  h.result('fork_limit_one', (await run('execute as @e[tag=parity_fr,limit=1] run scoreboard players add #fl fn 1')) + ' ' + await score('#fl'))
  await run('gamerule max_command_forks 65536')
  h.cmd('kill @e[tag=parity_fr]')

  // execute if|unless items|slots.
  h.cmd(`clear ${bot.username}`)
  h.cmd(`item replace entity ${bot.username} hotbar.0 with stone 5`)
  h.cmd(`item replace entity ${bot.username} container.10 with stone 7`)
  h.cmd(`item replace entity ${bot.username} armor.head with diamond_helmet`)
  h.cmd(`item replace entity ${bot.username} enderchest.3 with dirt 2`)
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z - 3} barrel{Items:[{Slot:0b,id:"minecraft:apple",count:5},{Slot:4b,id:"minecraft:stone",count:1}]}`)
  h.cmd(`setblock ${X + 4} ${Y + 1} ${Z - 3} stone`)
  await h.ticks(10)
  for (const [key, c] of [
    ['entity_container', `execute if items entity @s container.* stone`],
    ['entity_hotbar', `execute if items entity @s hotbar.0 minecraft:stone`],
    ['entity_mainhand', `execute if items entity @s weapon.mainhand stone`],
    ['entity_head', `execute if items entity @s armor.head *`],
    ['entity_feet', `execute if items entity @s armor.feet *`],
    ['entity_ender', `execute if items entity @s enderchest.* dirt`],
    ['entity_tag', `execute if items entity @s container.* #minecraft:stone_crafting_materials`],
    ['unless_entity', `execute unless items entity @s armor.feet *`],
    ['unless_entity_count', `execute unless items entity @s container.* stone`],
    ['block_all', `execute if items block ${X + 3} ${Y + 1} ${Z - 3} container.* *`],
    ['block_apple', `execute if items block ${X + 3} ${Y + 1} ${Z - 3} container.0 apple`],
    ['block_not_container', `execute if items block ${X + 4} ${Y + 1} ${Z - 3} container.* *`],
    ['slots_entity', `execute if slots entity @s container.*`],
    ['slots_block', `execute if slots block ${X + 3} ${Y + 1} ${Z - 3} container.*`],
    ['slots_armor', `execute if slots entity @s armor.*`],
    ['items_run', `execute if items entity @s weapon.mainhand stone run scoreboard players set #it fn 1`]
  ]) h.result(`cond_items_${key}`, await run(c))
  h.result('cond_items_run_score', await score('#it'))

  // data: deep merge, modify merge, from other sources, insert, get without a path.
  await run('data merge storage p:d {a:{b:1,c:2},l:[{x:1}]}')
  h.result('data_merge_deep', (await run('data merge storage p:d {a:{b:5}}')) + ' ' + await run('data get storage p:d a'))
  h.result('data_modify_merge', (await run('data modify storage p:d a merge value {d:4}')) + ' ' + await run('data get storage p:d a'))
  h.result('data_modify_merge_new', (await run('data modify storage p:d n merge value {e:1}')) + ' ' + await run('data get storage p:d n'))
  h.result('data_modify_merge_list', (await run('data modify storage p:d l[] merge value {y:2}')) + ' ' + await run('data get storage p:d l'))
  h.result('data_modify_from_block', (await run(`data modify storage p:d it set from block ${X + 3} ${Y + 1} ${Z - 3} Items[0].id`)) + ' ' + await run('data get storage p:d it'))
  h.result('data_modify_from_entity', (await run(`data modify storage p:d me set from entity @s SelectedItemSlot`)) + ' ' + await run('data get storage p:d me'))
  h.result('data_modify_insert', (await run('data modify storage p:d l insert 0 value {z:3}')) + ' ' + await run('data get storage p:d l'))
  h.result('data_modify_string', (await run('data modify storage p:d s set string storage p:m t')) + ' ' + await run('data get storage p:d s'))
  h.result('data_get_root', await run('data get storage p:d'))
  h.result('data_get_root_store', (await run('execute store result score #dg fn run data get storage p:d')) + ' ' + await score('#dg'))
  for (const v of ['45.0f', '0.1f', '1.0E10f', '0.0d', '1.0E-5d', '123456789.0d', '[1.5f,2.0f]', '[B;1b,2b]', '[I;1,2]', '[L;1L,2L]', '"it\'s"', `'say "hi"'`, '{"a b":1,c:[]}']) {
    await run(`data modify storage p:f v set value ${v}`)
    h.result(`data_format_${v}`, await run('data get storage p:f v'))
  }
  h.result('data_block_merge', (await run(`data merge block ${X + 3} ${Y + 1} ${Z - 3} {CustomName:"x"}`)) + ' ' + await run(`data get block ${X + 3} ${Y + 1} ${Z - 3} CustomName`))
  h.result('data_block_remove', (await run(`data remove block ${X + 3} ${Y + 1} ${Z - 3} Items[1]`)) + ' ' + await run(`data get block ${X + 3} ${Y + 1} ${Z - 3} Items`))
  h.result('data_player_modify', await run(`data modify entity @s Health set value 5f`))
})
