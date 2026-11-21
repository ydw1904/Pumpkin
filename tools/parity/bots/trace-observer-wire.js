// Tick trace: an observer / sticky piston clock pushes its observer while it is powered, next
// to wire. The observer unpowers itself in vanilla `ObserverBlock.onPlace` with flags 18, which
// skip shape updates, and the outer `setBlock` skips its own because the state changed under it.
// The wire must keep its straight shape instead of bending toward the observer for a tick.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1
// The wire next to the observer goes down after its neighbour, so it is placed straight in one packet.
const wires = [[0, 0], [0, 1], [0, 2], [0, 3], [1, 0], [1, -1], [1, 3], [2, 2], [2, 3]]

trace({
  name: 'observer-wire',
  setup: [
    `/setblock 2 ${Y} 1 sticky_piston[facing=north]`,
    ...wires.map(([x, z]) => `/setblock ${x} ${Y} ${z} redstone_wire`),
    `/setblock 2 ${Y} 0 observer[facing=east]`,
    // Starts the clock: the observer sees the block in front of it change.
    `/setblock 3 ${Y} 0 stone`,
    `/setblock 3 ${Y} 0 air`
  ],
  probes: {
    north_packets: { packets: [[2, Y, -1], [1, Y, -1]] },
    wire: { block: [1, Y, -1] },
    server_wire: { server: [1, Y, -1], states: ['redstone_wire[east=side]', 'redstone_wire[east=none]'] },
    server_observer: { server: [2, Y, -1], states: ['observer[powered=false]', 'observer[powered=true]', 'moving_piston', 'air'] }
  },
  ticks: 16
})
