// Command syntax datapacks lean on: datapack block tags in block predicates, item predicate
// component tests, /particle options, display entity NBT (matrix transformations, partial
// transformations, brightness), selector scores without a score, rotations given to command
// sources (rotated as, facing), SelectedItem in player NBT, /schedule with function tags and the
// tags sent to players after a reload, and datapack advancements (added, overriding a built-in
// one, out of the tree; reward functions; the reset sent on reload). An advancement that fails to
// parse makes vanilla fail the whole reload, so the unit tests cover that instead.
const fs = require('fs')
const path = require('path')
const { make } = require('./survival-lib')
const h = make('Datapacker')
const { bot } = h
const X = 6800; const Y = 200; const Z = 0

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
const tagPackets = []
bot._client.on('tags', (p) => tagPackets.push(p))
const advPackets = []
bot._client.on('advancements', (p) => advPackets.push(p))
const advIds = (packets) => packets.flatMap(p => p.advancementMapping.map(a => a.key))
const advEntry = (packets, id) => {
  for (const p of packets) {
    const a = p.advancementMapping.find(a => a.key === id)
    if (a) return a.value
  }
  return undefined
}
const blockTag = (p, name) => {
  const block = p.tags.find(t => t.tagType === 'minecraft:block')
  const tag = block && block.tags.find(t => t.tagName === name)
  return tag ? tag.entries.length : 'missing'
}
async function run (c, wait = 2) {
  msgs.length = 0
  bot.chat('/' + c)
  await h.waitFor(() => msgs.length > 0, 3000)
  await h.ticks(wait)
  return msgs.join('|') || 'none'
}

h.start(async () => {
  const pack = path.join(path.dirname(process.env.CMDS), 'world', 'datapacks', 'parity_dp')
  fs.mkdirSync(path.join(pack, 'data', 'p', 'tags', 'block'), { recursive: true })
  fs.mkdirSync(path.join(pack, 'data', 'p', 'tags', 'function'), { recursive: true })
  fs.mkdirSync(path.join(pack, 'data', 'p', 'function'), { recursive: true })
  fs.mkdirSync(path.join(pack, 'data', 'minecraft', 'tags', 'block'), { recursive: true })
  fs.writeFileSync(path.join(pack, 'pack.mcmeta'), JSON.stringify({ pack: { description: 'parity datapack commands', min_format: 121, max_format: 121 } }))
  fs.writeFileSync(path.join(pack, 'data', 'p', 'tags', 'block', 'stones.json'), JSON.stringify({ values: ['minecraft:stone', '#p:more'] }))
  fs.writeFileSync(path.join(pack, 'data', 'p', 'tags', 'block', 'more.json'), JSON.stringify({ values: ['minecraft:granite'] }))
  fs.writeFileSync(path.join(pack, 'data', 'minecraft', 'tags', 'block', 'wool.json'), JSON.stringify({ values: ['minecraft:stone'] }))
  fs.writeFileSync(path.join(pack, 'data', 'minecraft', 'tags', 'block', 'logs.json'), JSON.stringify({ replace: true, values: ['minecraft:stone'] }))
  fs.writeFileSync(path.join(pack, 'data', 'p', 'function', 'mark.mcfunction'), 'scoreboard players add #mark dp 1\n')
  fs.writeFileSync(path.join(pack, 'data', 'p', 'function', 'macro.mcfunction'), '$say $(x)\n')
  fs.writeFileSync(path.join(pack, 'data', 'p', 'tags', 'function', 'marks.json'), JSON.stringify({ values: ['p:mark'] }))
  const adv = path.join(pack, 'data', 'p', 'advancement')
  fs.mkdirSync(adv, { recursive: true })
  fs.mkdirSync(path.join(pack, 'data', 'minecraft', 'advancement', 'story'), { recursive: true })
  fs.writeFileSync(path.join(adv, 'root.json'), JSON.stringify({
    display: { icon: { id: 'minecraft:stick' }, title: 'Parity root', description: 'tab', background: 'minecraft:block/stone', show_toast: false, announce_to_chat: false },
    criteria: { tick: { trigger: 'minecraft:tick' } }
  }))
  fs.writeFileSync(path.join(adv, 'child.json'), JSON.stringify({
    parent: 'p:root',
    display: { icon: 'minecraft:diamond', title: { text: 'Parity child' }, description: 'granted', frame: 'goal' },
    criteria: { a: { trigger: 'minecraft:impossible' }, b: { trigger: 'minecraft:impossible' } },
    requirements: [['a', 'b']],
    rewards: { function: 'p:reward', experience: 3 }
  }))
  fs.writeFileSync(path.join(adv, 'orphan.json'), JSON.stringify({ parent: 'p:nope', criteria: { a: { trigger: 'minecraft:impossible' } } }))
  fs.writeFileSync(path.join(pack, 'data', 'minecraft', 'advancement', 'story', 'mine_stone.json'), JSON.stringify({
    parent: 'minecraft:story/root',
    display: { icon: 'minecraft:cobblestone', title: 'Overridden', description: 'x' },
    criteria: { x: { trigger: 'minecraft:impossible' } }
  }))
  fs.writeFileSync(path.join(pack, 'data', 'p', 'function', 'reward.mcfunction'), 'scoreboard players add #reward dp 1\n')

  await h.arena(X, Y, Z, 6)
  h.cmd(`gamemode creative ${bot.username}`)
  await h.ticks(10)
  // Count what the reload and the enable send.
  tagPackets.length = 0
  advPackets.length = 0
  await run('reload', 40)
  h.result('enable_after_reload', await run('datapack enable "file/parity_dp"', 40))
  // PlayerList.reloadResources sends the reloaded tags to every player.
  const reloaded = tagPackets[tagPackets.length - 1]
  h.result('reload_tags_sent', String(tagPackets.length))
  h.result('reload_tags_new', reloaded ? blockTag(reloaded, 'p:stones') : 'none')
  h.result('reload_tags_replaced', reloaded ? blockTag(reloaded, 'minecraft:logs') : 'none')
  await run('scoreboard objectives add dp dummy')
  const y = Y + 1

  // Datapack advancements: PlayerAdvancements.reload resends everything with reset set.
  h.result('adv_reset_sent', String(advPackets.some(p => p.reset)))
  await h.ticks(40)
  const ids = advIds(advPackets)
  h.result('adv_root_visible', String(ids.includes('p:root')))
  h.result('adv_child_visible', String(ids.includes('p:child')))
  const child = advEntry(advPackets, 'p:child')
  h.result('adv_child_parent', child ? String(child.parentId) : 'none')
  h.result('adv_child_requirements', child ? JSON.stringify(child.requirements) : 'none')
  const stone = advEntry(advPackets, 'minecraft:story/mine_stone')
  h.result('adv_override_requirements', stone ? JSON.stringify(stone.requirements) : 'none')
  h.result('adv_grant_child', await run(`advancement grant ${bot.username} only p:child a`))
  h.result('adv_reward_ran', await run('scoreboard players get #reward dp'))
  h.result('adv_grant_child_again', await run(`advancement grant ${bot.username} only p:child`))
  h.result('adv_grant_orphan', await run(`advancement grant ${bot.username} only p:orphan`))
  h.result('adv_override_old_criterion', await run(`advancement grant ${bot.username} only minecraft:story/mine_stone get_stone`))
  h.result('adv_from_root', await run(`advancement revoke ${bot.username} from p:root`))
  // Progress survives a reload.
  await run(`advancement grant ${bot.username} only p:child`)
  await run('reload', 40)
  h.result('adv_kept_after_reload', await run(`advancement grant ${bot.username} only p:child`))

  // Block predicates with datapack tags: a new tag, one nesting another, an extended vanilla
  // tag and a replaced one.
  // The console's feedback reaches the bot as an op broadcast; wait for it so it doesn't land
  // in the next result when the server lags behind after the reload.
  msgs.length = 0
  h.cmd(`setblock ${X} ${y} ${Z} granite`)
  h.cmd(`setblock ${X + 1} ${y} ${Z} stone`)
  h.cmd(`setblock ${X + 2} ${y} ${Z} oak_log`)
  await h.waitFor(() => msgs.filter(m => m.includes('commands.setblock.success')).length >= 3, 10000)
  await h.ticks(5)
  h.result('tag_nested', await run(`execute if block ${X} ${y} ${Z} #p:stones`))
  h.result('tag_direct', await run(`execute if block ${X + 1} ${y} ${Z} #p:stones`))
  h.result('tag_extended', await run(`execute if block ${X + 1} ${y} ${Z} #minecraft:wool`))
  h.result('tag_replaced_old', await run(`execute if block ${X + 2} ${y} ${Z} #minecraft:logs`))
  h.result('tag_replaced_new', await run(`execute if block ${X + 1} ${y} ${Z} #minecraft:logs`))
  h.result('tag_unknown', await run(`execute if block ${X} ${y} ${Z} #p:nope`))
  h.result('tag_fill_filter', await run(`fill ${X} ${y + 1} ${Z} ${X + 2} ${y + 1} ${Z} glass replace #p:stones`))

  // /schedule: function tags, macros, clearing and the returned time.
  await run('scoreboard players set #mark dp 0')
  h.result('schedule_tag', (await run('schedule function #p:marks 2t')).replace(/\d{3,}/g, 'T'))
  h.result('schedule_macro', await run('schedule function p:macro 5t'))
  h.result('schedule_unknown', await run('schedule function p:nope 5t'))
  await h.ticks(10)
  h.result('schedule_tag_ran', await run('scoreboard players get #mark dp'))
  await run('schedule function p:mark 100t')
  await run('schedule function p:mark 200t append')
  h.result('schedule_clear', await run('schedule clear p:mark'))
  h.result('schedule_clear_again', await run('schedule clear p:mark'))

  // Item predicates with component tests.
  await run(`item replace entity ${bot.username} hotbar.0 with stick[custom_data={car:1b,color:"red"}]`)
  await run(`item replace entity ${bot.username} hotbar.1 with diamond_sword[damage=5]`)
  await run(`item replace entity ${bot.username} hotbar.2 with paper[custom_name={text:"Named",italic:false}] 3`)
  for (const [key, p] of [
    ['custom_data_partial', '*[custom_data~{car:1b}]'],
    ['custom_data_wrong', '*[custom_data~{car:2b}]'],
    ['custom_data_exact', '*[custom_data={car:1b}]'],
    ['custom_data_string', '*[custom_data~"{car:1b}"]'],
    ['not_custom_data', 'stick[!custom_data]'],
    ['damage_range', '*[damage~{damage:{min:1,max:9}}]'],
    ['damage_exact', '*[damage=5]'],
    ['exists', '*[damage]'],
    ['alternatives', '*[damage=5|custom_data]'],
    ['and', '*[custom_name,count=3]'],
    ['count_range', '*[count~{min:2}]'],
    ['default_component', '*[max_stack_size=64]'],
    ['unknown', '*[not_a_component]'],
    ['tag_filter', '#swords[damage]']
  ]) h.result(`items_${key}`, await run(`execute if items entity ${bot.username} hotbar.* ${p}`))

  // Selected item in player NBT, the nbt= selector and data get.
  bot.setQuickBarSlot(0)
  await h.ticks(5)
  h.result('selected_item_nbt', await run(`execute if entity @s[nbt={SelectedItem:{id:"minecraft:stick"}}]`))
  h.result('selected_item_data', await run('data get entity @s SelectedItem.id'))

  // Selector scores need a score.
  h.result('scores_missing', await run('execute if entity @s[scores={dp=0}]'))
  await run('scoreboard players set @s dp 0')
  h.result('scores_set', await run('execute if entity @s[scores={dp=0}]'))

  // Display entities: matrix transformations, partial transformations and brightness.
  h.cmd(`summon block_display ${X} ${y + 2} ${Z} {Tags:["dp_m"],block_state:{id:"minecraft:oak_log",properties:{axis:"x"}},transformation:[2f,0f,0f,0.5f,0f,2f,0f,1f,0f,0f,2f,0f,0f,0f,0f,1f]}`)
  h.cmd(`summon block_display ${X} ${y + 2} ${Z} {Tags:["dp_r"],transformation:[0f,0f,1f,0f,0f,1.5f,0f,0f,-1f,0f,0f,0f,0f,0f,0f,1f]}`)
  h.cmd(`summon block_display ${X} ${y + 2} ${Z} {Tags:["dp_p"],transformation:{scale:[3f,3f,3f]}}`)
  h.cmd(`summon block_display ${X} ${y + 2} ${Z} {Tags:["dp_q"],transformation:{translation:[0f,0f,0f],scale:[1f,1f,1f],left_rotation:[0f,0f,0f,2f],right_rotation:{angle:1.5707964f,axis:[0f,1f,0f]}}}`)
  h.cmd(`summon text_display ${X} ${y + 2} ${Z} {Tags:["dp_b"],brightness:{block:15,sky:3},view_range:0.5d}`)
  await h.ticks(5)
  h.result('display_matrix', await run('data get entity @e[tag=dp_m,limit=1] transformation'))
  h.result('display_rotated_matrix', await run('data get entity @e[tag=dp_r,limit=1] transformation'))
  h.result('display_partial', await run('data get entity @e[tag=dp_p,limit=1] transformation.scale'))
  h.result('display_quaternions', await run('data get entity @e[tag=dp_q,limit=1] transformation'))
  h.result('display_block_state', await run('data get entity @e[tag=dp_m,limit=1] block_state'))
  h.result('display_brightness', await run('data get entity @e[tag=dp_b,limit=1] brightness'))
  h.result('display_view_range', await run('data get entity @e[tag=dp_b,limit=1] view_range'))
  h.result('display_start_interpolation', await run('data get entity @e[tag=dp_b,limit=1] start_interpolation'))

  // Rotations given to command sources.
  h.cmd(`summon armor_stand ${X}.5 ${y} ${Z + 3}.5 {Tags:["dp_rot"],NoGravity:1b,Rotation:[90f,30f]}`)
  h.cmd(`summon marker ${X}.5 ${y} ${Z}.5 {Tags:["dp_mk"]}`)
  await h.ticks(5)
  await run(`execute positioned ${X}.5 ${y} ${Z}.5 rotated as @e[tag=dp_rot,limit=1] run tp @e[tag=dp_mk,limit=1] ^ ^ ^2`)
  h.result('rotated_as', await run('data get entity @e[tag=dp_mk,limit=1] Pos'))
  await run(`execute positioned ${X}.5 ${y} ${Z}.5 facing ${X + 3}.5 ${y + 3} ${Z}.5 run tp @e[tag=dp_mk,limit=1] ^ ^ ^2`)
  h.result('facing_pos', await run('data get entity @e[tag=dp_mk,limit=1] Pos'))
  await run(`execute as @e[tag=dp_rot,limit=1] at @s anchored eyes facing ${X}.5 ${y} ${Z - 3}.5 run tp @e[tag=dp_mk,limit=1] ^ ^ ^1`)
  h.result('facing_anchored', await run('data get entity @e[tag=dp_mk,limit=1] Pos'))
  await run(`execute at @e[tag=dp_rot,limit=1] run tp @e[tag=dp_mk,limit=1] ^ ^ ^1`)
  h.result('at_rotation', await run('data get entity @e[tag=dp_mk,limit=1] Pos'))

  // /particle options.
  for (const [key, p] of [
    ['dust', 'dust{color:[1.0,0.0,0.0],scale:2}'],
    ['dust_int', 'dust{color:16711680,scale:10}'],
    ['dust_missing', 'dust'],
    ['transition', 'dust_color_transition{from_color:[1.0,0.0,0.0],to_color:[0.0,0.0,1.0],scale:1}'],
    ['block', 'block{block_state:"minecraft:stone"}'],
    ['block_props', 'block{block_state:{id:"minecraft:oak_log",properties:{axis:"x"}}}'],
    ['block_missing', 'block'],
    ['item', 'item{item:"minecraft:diamond"}'],
    ['item_compound', 'item{item:{id:"minecraft:diamond",count:2}}'],
    ['entity_effect', 'entity_effect{color:[1.0,0.0,0.0,1.0]}'],
    ['effect_default', 'effect'],
    ['shriek', 'shriek{delay:5}'],
    ['plain', 'flame'],
    ['not_a_map', 'dust[1]']
  ]) h.result(`particle_${key}`, await run(`particle ${p} ${X} ${y + 1} ${Z}`))

  h.cmd('kill @e[tag=dp_m]')
  h.cmd('kill @e[tag=dp_r]')
  h.cmd('kill @e[tag=dp_p]')
  h.cmd('kill @e[tag=dp_q]')
  h.cmd('kill @e[tag=dp_b]')
  h.cmd('kill @e[tag=dp_rot]')
  h.cmd('kill @e[tag=dp_mk]')
})
