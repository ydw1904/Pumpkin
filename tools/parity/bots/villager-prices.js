// Villager prices: demand raising the first cost, gossip and Hero of the Village lowering it, what
// the server actually charges, uses and the out-of-stock flag, and shift-click trading. The
// villagers have NoAI, like shop villagers on servers.
const { make } = require('./survival-lib')
const h = make('Haggler')
const { bot } = h
const X = 3100; const Y = 200; const Z = 0

const lists = []
bot._client.on('trade_list', (p) => lists.push(p))
const count = (s) => s ? (s.itemCount ?? s.count ?? 0) : 0
const offerStr = (t) => `${count(t.inputItem1)}+${count(t.inputItem2)}->${count(t.outputItem)} uses=${t.nbTradeUses}/${t.maximumNbTradeUses} special=${t.specialPrice} demand=${t.demand} disabled=${t.tradeDisabled}`
const offer = (buy, n, sell, m, extra = '', maxUses = 4) => `{buy:{id:"minecraft:${buy}",count:${n}},sell:{id:"minecraft:${sell}",count:${m}},maxUses:${maxUses},xp:2,priceMultiplier:0.05f${extra}}`

const villager = async (offers, extraNbt = '') => {
  h.cmd('kill @e[type=villager]')
  await h.waitFor(() => !Object.values(bot.entities).some(e => e.name === 'villager'), 3000)
  h.cmd(`summon villager ${X + 2.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,VillagerData:{profession:"minecraft:farmer",level:1,type:"minecraft:plains"},Offers:{Recipes:[${offers.join(',')}]}${extraNbt}}`)
  await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'villager'), 5000)
  await h.ticks(10)
  return Object.values(bot.entities).find(e => e.name === 'villager')
}
// Opens the villager with only `items` in the inventory and reports the offers it sends.
const look = async (key, entity, items) => {
  await h.fill(items)
  const before = lists.length
  const w = await bot.openVillager(entity)
  await h.waitFor(() => lists.length > before, 5000)
  h.result(key, lists[lists.length - 1].trades.map(offerStr).join(' | '))
  return w
}
// Selecting an offer moves the payment in from the inventory; reports whether a result appears.
const select = async (w, index = 0) => {
  bot._client.write('select_trade', { slot: index })
  await h.ticks(3)
  await bot._syncWindow(w)
  return `paid=${h.stackStr(w.slots[0])} result=${h.stackStr(w.slots[2])}`
}
const shut = async (w) => { w.close(); await h.ticks(5) }

h.start(async () => {
  await h.arena(X, Y, Z, 6)
  h.cmd('time set 1000')
  const uuid = bot.player.uuid.replace(/-/g, '')
  const ints = [0, 8, 16, 24].map(i => (parseInt(uuid.slice(i, i + 8), 16) | 0)).join(',')

  // Demand 10 at multiplier 0.05 raises 20 wheat to 30.
  let v = await villager([offer('wheat', 20, 'emerald', 1, ',demand:10')])
  let w = await look('demand', v, { 0: 'wheat 29' })
  h.result('demand.with_29', await select(w))
  await shut(w)
  w = await look('demand.again', v, { 0: 'wheat 30' })
  h.result('demand.with_30', await select(w))
  await shut(w)

  // Reputation: 20 major positive (x5), 25 minor positive, 10 trading.
  v = await villager([offer('wheat', 20, 'emerald', 1), offer('emerald', 5, 'bread', 6)],
    `,Gossips:[{Type:"major_positive",Value:20,Target:[I;${ints}]},{Type:"minor_positive",Value:25,Target:[I;${ints}]},{Type:"trading",Value:10,Target:[I;${ints}]}]`)
  w = await look('gossip_positive', v, { 0: 'wheat 64' })
  h.result('gossip_positive.with_64', await select(w))
  await shut(w)

  v = await villager([offer('wheat', 20, 'emerald', 1), offer('emerald', 5, 'bread', 6)],
    `,Gossips:[{Type:"major_negative",Value:20,Target:[I;${ints}]},{Type:"minor_negative",Value:100,Target:[I;${ints}]}]`)
  w = await look('gossip_negative', v, { 0: 'wheat 64' })
  await shut(w)

  v = await villager([offer('wheat', 20, 'emerald', 1), offer('emerald', 5, 'bread', 6), offer('emerald', 1, 'bread', 6)])
  h.cmd(`effect give ${bot.username} hero_of_the_village 100 0`)
  await h.ticks(5)
  w = await look('hero_1', v, { 0: 'wheat 64' })
  await shut(w)
  h.cmd(`effect give ${bot.username} hero_of_the_village 100 4`)
  await h.ticks(5)
  w = await look('hero_5', v, { 0: 'wheat 64' })
  await shut(w)
  h.cmd(`effect clear ${bot.username}`)
  await h.ticks(5)
  w = await look('hero_cleared', v, { 0: 'wheat 64' })
  await shut(w)

  // Shift-clicking the result trades as long as payment and stock last; four uses lock it.
  v = await villager([offer('wheat', 2, 'emerald', 1)])
  w = await look('uses.start', v, { 0: 'wheat 64' })
  h.result('uses.selected', await select(w))
  await h.click(2, 0, 1)
  h.result('uses.after_shift', `${h.win(0, 3)} inv=${h.invStr()}`)
  await shut(w)
  w = await look('uses.reopened', v, { 0: 'wheat 64' })
  h.result('uses.reselected', await select(w))
  await shut(w)
})
