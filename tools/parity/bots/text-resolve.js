// Text components the server fills in before sending (score, selector and NBT contents in
// /tellraw), and the commands that take a loot table, item modifier or predicate written inline.
const { make } = require('./survival-lib')
const h = make('Resolver')
const { bot } = h
const X = 3700; const Y = 200; const Z = 0

const chat = []
bot.on('message', (m) => chat.push(m))

// Runs a console command and returns the first chat line it makes the bot see, or the text of
// its hover tooltip.
async function seen (cmd, hover = false) {
  const c0 = chat.length
  h.cmd(cmd)
  if (!await h.waitFor(() => chat.length > c0, 5000)) return '<none>'
  await h.sleep(100)
  const json = chat[c0].json
  return JSON.stringify(hover ? json.hover_event && json.hover_event.value && json.hover_event.value.text : chat[c0].toString())
}

h.start(async () => {
  await h.arena(X, Y, Z, 4)
  h.cmd('scoreboard objectives add kills dummy')
  h.cmd(`scoreboard players set ${bot.username} kills 7`)
  h.cmd('scoreboard players set Fake kills 3')
  h.cmd('data modify storage parity:text value set value "hi"')
  await h.ticks(10)

  h.result('tellraw.selector', await seen('tellraw @a {"selector":"@p"}'))
  h.result('tellraw.score_wildcard', await seen('tellraw @a ["s=",{"score":{"name":"*","objective":"kills"}}]'))
  h.result('tellraw.score_name', await seen('tellraw @a ["s=",{"score":{"name":"Fake","objective":"kills"}}]'))
  h.result('tellraw.score_selector', await seen('tellraw @a ["s=",{"score":{"name":"@p","objective":"kills"}}]'))
  h.result('tellraw.score_missing', await seen('tellraw @a ["s=",{"score":{"name":"Nobody","objective":"kills"}}]'))
  h.result('tellraw.nbt_storage', await seen('tellraw @a ["n=",{"nbt":"value","storage":"parity:text"}]'))
  h.result('tellraw.nbt_entity', await seen('tellraw @a ["n=",{"nbt":"SelectedItemSlot","entity":"@p"}]'))
  h.result('tellraw.nbt_interpret', await seen('tellraw @a ["n=",{"nbt":"value","storage":"parity:text","interpret":true}]'))
  h.result('tellraw.hover_resolved', await seen('tellraw @a {"text":"h","hover_event":{"action":"show_text","value":{"selector":"@p"}}}', true))
  // The whole component: the name's show_entity hover carries the type and the uuid.
  const c0 = chat.length
  h.cmd('tellraw @a {"selector":"@p"}')
  await h.waitFor(() => chat.length > c0, 5000)
  const sorted = (v) => Array.isArray(v)
    ? v.map(sorted)
    : (v && typeof v === 'object' ? Object.fromEntries(Object.keys(v).sort().map(k => [k, sorted(v[k])])) : v)
  h.result('tellraw.selector_json', JSON.stringify(sorted(chat[c0] && chat[c0].json)))

  // Signs resolve their lines when loaded with allow_op_features, lecterns their book and
  // text displays their text.
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z} oak_sign{allow_op_features:1b,front_text:{messages:[{score:{name:"Fake",objective:"kills"}},{selector:"@p"},"",""]}}`)
  h.cmd(`setblock ${X + 1} ${Y + 1} ${Z + 1} oak_sign{front_text:{messages:[{score:{name:"Fake",objective:"kills"}},"","",""]}}`)
  h.cmd(`setblock ${X + 2} ${Y + 1} ${Z} lectern[has_book=true]{Book:{id:"minecraft:written_book",count:1,components:{"minecraft:written_book_content":{title:"t",author:"a",pages:[{score:{name:"Fake",objective:"kills"}}]}}}}`)
  h.cmd(`summon text_display ${X} ${Y + 2} ${Z} {Tags:["resolve"],text:{score:{name:"Fake",objective:"kills"}}}`)
  await h.ticks(5)
  // `data get` answers the console, so the stored data is shown to the bot through tellraw.
  const nbt = (path, source, interpret = false) =>
    seen(`tellraw @a ["v=",{"nbt":${JSON.stringify(path)},${source},"interpret":${interpret}}]`)
  const display = '"entity":"@e[type=text_display,tag=resolve,limit=1]"'
  for (const i of [0, 1]) {
    h.result(`sign.op_features.${i}`, await nbt(`front_text.messages[${i}]`, `"block":"${X + 1} ${Y + 1} ${Z}"`, true))
  }
  h.result('sign.op_features.raw', await nbt('front_text.messages[1]', `"block":"${X + 1} ${Y + 1} ${Z}"`))
  h.result('sign.plain', await nbt('front_text.messages[0]', `"block":"${X + 1} ${Y + 1} ${Z + 1}"`))
  h.result('lectern.book', await nbt('Book.components', `"block":"${X + 2} ${Y + 1} ${Z}"`))
  h.result('text_display.summon', await nbt('text', display, true))
  h.cmd('data merge entity @e[type=text_display,tag=resolve,limit=1] {text:{selector:"@s"}}')
  await h.ticks(10)
  h.result('text_display.merge', await nbt('text', display, true))

  await h.fill({ 0: 'stone 1' })
  h.cmd(`item modify entity ${bot.username} hotbar.0 {type:"minecraft:set_count",count:5}`)
  await h.waitFor(() => (h.inv().stone || 0) === 5, 3000)
  h.result('item_modify.set_count', h.invStr())
  h.cmd(`item modify entity ${bot.username} hotbar.0 [{type:"minecraft:set_count",count:2},{type:"minecraft:set_count",count:3,add:true}]`)
  await h.sleep(1500)
  h.result('item_modify.sequence', h.invStr())
  h.result('item_modify.empty_slot', await seen(`item modify entity ${bot.username} hotbar.1 {type:"minecraft:set_count",count:5}`))

  // `/item replace ... from` copies slots, optionally through a modifier; slot ranges take one
  // item each while the source lasts.
  await h.fill({ 0: 'stone 3' })
  h.cmd(`setblock ${X + 3} ${Y + 1} ${Z} chest`)
  await h.ticks(10)
  h.result('item_replace.from_entity', await seen(`item replace entity ${bot.username} hotbar.1 from entity ${bot.username} hotbar.0`))
  h.result('item_replace.from_modifier', await seen(`item replace entity ${bot.username} hotbar.2 from entity ${bot.username} hotbar.0 {type:"minecraft:set_count",count:99}`))
  h.result('item_replace.to_block', await seen(`item replace block ${X + 3} ${Y + 1} ${Z} container.* from entity ${bot.username} hotbar.*`))
  h.result('item_replace.from_block', await seen(`item replace entity ${bot.username} hotbar.5 from block ${X + 3} ${Y + 1} ${Z} container.2`))
  h.result('item_replace.no_source', await seen(`item replace entity ${bot.username} hotbar.5 from block ${X + 1} ${Y + 1} ${Z} container.0`))
  h.result('item_replace.with_range', await seen(`item replace entity ${bot.username} hotbar.* with minecraft:dirt 2`))
  await h.sleep(1000)
  h.result('item_replace.inventory', h.invStr())
  h.result('item_replace.chest', await nbt('Items', `"block":"${X + 3} ${Y + 1} ${Z}"`))

  await h.clear()
  h.cmd(`loot give ${bot.username} loot {pools:[{rolls:1,entries:[{type:"minecraft:item",name:"minecraft:diamond"}]}]}`)
  await h.waitFor(() => h.inv().diamond, 3000)
  h.result('loot.inline_table', h.invStr())

  h.result('predicate.inline_pass', await seen('execute if predicate {type:"minecraft:random_chance",chance:1.0f} run tellraw @a "pass"'))
  h.result('predicate.inline_fail', await seen('execute if predicate {type:"minecraft:random_chance",chance:0.0f} run tellraw @a "fail"'))
})
