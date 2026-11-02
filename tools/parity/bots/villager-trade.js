// Trading with a villager: offers sent on open, payment and results, the server still ticking
// after a trade, uses counted, experience orbs, and the level up with its new offers.
const { make } = require('./survival-lib')
const h = make('Trader')
const { bot } = h
const X = 1900; const Y = 200; const Z = 0
const OFFERS = '{Recipes:[{buy:{id:"minecraft:wheat",count:20},sell:{id:"minecraft:emerald",count:1},maxUses:16,xp:2,priceMultiplier:0.05f},' +
  '{buy:{id:"minecraft:emerald",count:1},sell:{id:"minecraft:bread",count:6},maxUses:16,xp:1,priceMultiplier:0.05f}]}'

const offers = []
bot._client.on('trade_list', (p) => offers.push(p))

h.start(async () => {
  await h.arena(X, Y, Z, 10)
  h.cmd('time set 1000')
  await h.clear()
  h.cmd(`summon villager ${X + 2.5} ${Y + 1} ${Z + 0.5} {NoAI:1b,VillagerData:{profession:"minecraft:farmer",level:1,type:"minecraft:plains"},Offers:${OFFERS}}`)
  await h.waitFor(() => Object.values(bot.entities).some(e => e.name === 'villager'), 5000)
  await h.ticks(10)
  await h.give('wheat', 64)
  await h.give('wheat', 64)
  await h.give('emerald', 2)
  const villager = await bot.openVillager(Object.values(bot.entities).find(e => e.name === 'villager'))
  await h.waitFor(() => villager.trades && villager.trades.length > 0, 5000)
  h.result('trades', villager.trades.map(t => `${t.inputItem1.name}x${t.inputItem1.count}->${t.outputItem.name}x${t.outputItem.count}`).join(' '))
  // A real client draws offers flagged as out of stock crossed out.
  h.result('offers_flagged_out_of_stock', offers[0].trades.filter(t => t.tradeDisabled).length)

  await bot.trade(villager, 0, 2).catch((e) => h.result('trade_error', e.message.slice(0, 80)))
  await h.ticks(10)
  // Completing a trade once deadlocked the tick; block changes only reach clients from the tick.
  h.cmd(`setblock ${X - 2} ${Y + 1} ${Z} gold_block`)
  h.result('server_ticks_after_trade', await h.waitFor(() => h.blockName(X - 2, Y + 1, Z) === 'gold_block', 10000))

  // Three more wheat trades take the villager to 10 xp: journeyman offers appear.
  await bot.trade(villager, 0, 3).catch((e) => h.result('trade_error', e.message.slice(0, 80)))
  await h.ticks(30)
  const last = offers[offers.length - 1]
  h.result('villager_level', last.villagerLevel)
  h.result('offer_count_after_level_up', last.trades.length)
  h.result('wheat_uses', last.trades[0].nbTradeUses)
  h.result('player_got_xp', bot.experience.level > 0 || bot.experience.progress > 0 || h.orbs().length > 0)
  villager.close()
})
