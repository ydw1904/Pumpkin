// Tick trace: an observer / sticky piston clock gives two upward sticky pistons 0-tick pulses
// (extend and retract block events in the same tick). The client must end each pulse with the
// retracting `moving_piston` it was sent, not the stale `extended=true` state (vanilla
// `ChunkHolder.broadcastChanges` sends a position's current state), and the wire's neighbour
// updates must run in vanilla's `HashSet` order so both pistons get their events.
const { trace, PLATFORM_Y } = require('./steptrace')
const Y = PLATFORM_Y + 1
const wires = [[-1, 0], [0, 0], [0, 1], [0, 2], [0, 3], [1, -1], [1, 0], [1, 3], [2, 2], [2, 3]]

trace({
  name: 'piston-zero-tick',
  setup: [
    `/setblock -2 ${Y} 0 sticky_piston[facing=up]`,
    `/setblock 1 ${Y} -2 sticky_piston[facing=up]`,
    `/setblock 2 ${Y} 1 sticky_piston[facing=north]`,
    ...wires.map(([x, z]) => `/setblock ${x} ${Y} ${z} redstone_wire`),
    `/setblock 2 ${Y} 0 observer[facing=east]`,
    // Starts the clock: the observer sees the block in front of it change.
    `/setblock 3 ${Y} 0 stone`,
    `/setblock 3 ${Y} 0 air`
  ],
  probes: {
    a_packets: { packets: [[-2, Y, 0], [-2, Y + 1, 0]] },
    b_packets: { packets: [[1, Y, -2], [1, Y + 1, -2]] },
    // One probe per chunk: the order of packets for different chunks isn't compared.
    observer_north_packets: { packets: [[2, Y, -1]] },
    observer_south_packets: { packets: [[2, Y, 0], [2, Y, 1]] },
    a: { block: [-2, Y, 0] },
    b: { block: [1, Y, -2] },
    server_a: { server: [-2, Y, 0], states: ['sticky_piston[extended=false]', 'sticky_piston[extended=true]', 'moving_piston'] },
    server_b: { server: [1, Y, -2], states: ['sticky_piston[extended=false]', 'sticky_piston[extended=true]', 'moving_piston'] },
    server_observer: { server: [2, Y, 0], states: ['observer[powered=false]', 'observer[powered=true]', 'moving_piston', 'piston_head'] }
  },
  ticks: 16
})
