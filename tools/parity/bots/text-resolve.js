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

  await h.fill({ 0: 'stone 1' })
  h.cmd(`item modify entity ${bot.username} hotbar.0 {type:"minecraft:set_count",count:5}`)
  await h.waitFor(() => (h.inv().stone || 0) === 5, 3000)
  h.result('item_modify.set_count', h.invStr())
  h.cmd(`item modify entity ${bot.username} hotbar.0 [{type:"minecraft:set_count",count:2},{type:"minecraft:set_count",count:3,add:true}]`)
  await h.sleep(1500)
  h.result('item_modify.sequence', h.invStr())
  h.result('item_modify.empty_slot', await seen(`item modify entity ${bot.username} hotbar.1 {type:"minecraft:set_count",count:5}`))

  await h.clear()
  h.cmd(`loot give ${bot.username} loot {pools:[{rolls:1,entries:[{type:"minecraft:item",name:"minecraft:diamond"}]}]}`)
  await h.waitFor(() => h.inv().diamond, 3000)
  h.result('loot.inline_table', h.invStr())

  h.result('predicate.inline_pass', await seen('execute if predicate {type:"minecraft:random_chance",chance:1.0f} run tellraw @a "pass"'))
  h.result('predicate.inline_fail', await seen('execute if predicate {type:"minecraft:random_chance",chance:0.0f} run tellraw @a "fail"'))
})
