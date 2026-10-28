#!/usr/bin/env python3
"""Generates the `pumpkin-parity` datapack of block-based GameTests from the specs in tests.py.

Each test is a structure with one start test block at (0, 1, 0) on a stone floor. The start
block powers whatever sits next to it. Tests pass either directly, when the mechanic itself
powers an accept block, or through a check: a repeater line from the start block fires a
command block after a delay, which runs a function that powers the accept block or a fail
block named after the condition that didn't hold.

    python tools/parity/build_pack.py --out <dir>
"""

import argparse
import gzip
import importlib.util
import json
import shutil
import struct
from pathlib import Path

# Vanilla 26.3 world_version and data pack format, from the server jar's version.json.
DATA_VERSION = 5023
PACK_FORMAT = 121
NAMESPACE = "pumpkin_parity"
FLOOR = "minecraft:stone"
REPEATER_GAME_TICKS = 8  # A repeater at delay 4 holds the signal for 4 redstone ticks.


class Byte(int):
    pass


def _name(out, name):
    data = name.encode()
    out += struct.pack(">H", len(data)) + data


def _tag_id(value):
    if isinstance(value, Byte):
        return 1
    if isinstance(value, bool):
        return 1
    if isinstance(value, int):
        return 3
    if isinstance(value, str):
        return 8
    if isinstance(value, list):
        return 9
    if isinstance(value, dict):
        return 10
    raise TypeError(f"Unsupported NBT value {value!r}")


def _payload(out, value):
    tag = _tag_id(value)
    if tag == 1:
        out += struct.pack(">b", int(value))
    elif tag == 3:
        out += struct.pack(">i", value)
    elif tag == 8:
        _name(out, value)
    elif tag == 9:
        element = _tag_id(value[0]) if value else 0
        out += struct.pack(">bi", element, len(value))
        for item in value:
            _payload(out, item)
    else:
        for key, item in value.items():
            out += struct.pack(">b", _tag_id(item))
            _name(out, key)
            _payload(out, item)
        out += b"\x00"


def write_nbt(path, root):
    out = bytearray(b"\x0a")
    _name(out, "")
    _payload(out, root)
    path.write_bytes(gzip.compress(bytes(out)))


def block_id(state):
    """Splits `minecraft:x[a=b,c=d]` into (name, properties)."""
    if "[" not in state:
        return state, {}
    name, props = state[:-1].split("[", 1)
    return name, dict(p.split("=", 1) for p in props.split(","))


class Test:
    def __init__(self, name, max_ticks=100, description=""):
        self.name = name
        self.max_ticks = max_ticks
        self.description = description
        self.blocks = {}
        self.checks = []
        self.check_delay = 0
        self.actions = {}
        self.set((0, 1, 0), "minecraft:test_block[mode=start]", self._test_block_nbt("start"))

    @staticmethod
    def _test_block_nbt(mode, message=""):
        return {"id": "minecraft:test_block", "mode": mode, "message": message, "powered": Byte(0)}

    def set(self, pos, state, nbt=None):
        self.blocks[tuple(pos)] = (state, nbt)
        return self

    def accept(self, pos):
        return self.set(pos, "minecraft:test_block[mode=accept]", self._test_block_nbt("accept"))

    def fail(self, pos, message):
        return self.set(pos, "minecraft:test_block[mode=fail]", self._test_block_nbt("fail", message))

    def container(self, pos, state, items):
        name, _ = block_id(state)
        slots = [{"Slot": Byte(slot), "id": item, "count": count} for slot, (item, count) in items.items()]
        return self.set(pos, state, {"id": name, "Items": slots})

    def command(self, pos, command, facing="up"):
        nbt = {
            "id": "minecraft:command_block",
            "Command": command,
            "auto": Byte(0),
            "powered": Byte(0),
            "TrackOutput": Byte(0),
            "conditionMet": Byte(0),
            "SuccessCount": 0,
        }
        return self.set(pos, f"minecraft:command_block[conditional=false,facing={facing}]", nbt)

    def setup(self, *placements):
        """Runs `/setblock` for each (pos, state) when the test starts, in the order given."""
        return self.after(0, *placements)

    def after(self, repeaters, *placements):
        """Runs `/setblock` for each (pos, state) once `repeaters` delay-4 repeaters have carried
        the start signal along x=0. Each setblock updates the neighbours like the command does."""
        self.actions.setdefault(repeaters, []).extend(placements)
        return self

    def check_after(self, repeaters, *conditions):
        """Checks `conditions` (pos, state, description) once `repeaters` delay-4 repeaters have
        carried the start signal along x=0 to a command block."""
        self.check_delay = repeaters
        self.checks = list(conditions)
        return self

    @staticmethod
    def _rel(pos, origin):
        return "~{} ~{} ~{}".format(pos[0] - origin[0], pos[1] - origin[1], pos[2] - origin[2])

    def _action_function(self, cmd, placements):
        return "".join(f"setblock {self._rel(pos, cmd)} {state}\n" for pos, state in placements)

    def _build_timeline(self):
        """Lays out the delay line and returns the functions it runs, keyed by path."""
        functions = {}
        if 0 in self.actions:
            cmd = (1, 1, 0)
            if cmd in self.blocks:
                raise ValueError(f"{self.name}: setup needs (1, 1, 0) free for its command block")
            functions[f"action/{self.name}_0"] = self._action_function(cmd, self.actions[0])
            self.command(cmd, f"function {NAMESPACE}:action/{self.name}_0")
        if not self.checks:
            return functions

        timed = sorted(d for d in self.actions if d > 0)
        if timed and timed[-1] >= self.check_delay:
            raise ValueError(f"{self.name}: the check has to come after the last action")
        z, placed = 1, 0
        for delay in timed + [self.check_delay]:
            for _ in range(delay - placed):
                # A diode's facing points at its input, so these carry the signal towards +z.
                self.set((0, 1, z), "minecraft:repeater[delay=4,facing=north,locked=false,powered=false]")
                z += 1
            placed = delay
            if delay == self.check_delay:
                break
            # The repeater strongly powers this block, which fires the command block beside it and
            # feeds the next repeater.
            self.set((0, 1, z), FLOOR)
            cmd = (1, 1, z)
            functions[f"action/{self.name}_{delay}"] = self._action_function(cmd, self.actions[delay])
            self.command(cmd, f"function {NAMESPACE}:action/{self.name}_{delay}")
            z += 1

        cmd = (0, 1, z)
        self.command(cmd, f"function {NAMESPACE}:check/{self.name}")
        self.accept((0, 3, z))
        lines = []
        for i, (pos, state, description) in enumerate(self.checks):
            self.fail((0, 3, z + 1 + i), f"expected {description}")
            lines.append(
                f"execute unless block {self._rel(pos, cmd)} {state} run setblock ~ ~1 ~{1 + i} minecraft:redstone_block"
            )
        conditions = " ".join(f"if block {self._rel(p, cmd)} {s}" for p, s, _ in self.checks)
        lines.append(f"execute {conditions} run setblock ~ ~1 ~ minecraft:redstone_block")
        functions[f"check/{self.name}"] = "\n".join(lines) + "\n"
        return functions

    def build(self, data_dir):
        functions = self._build_timeline()
        xs, ys, zs = zip(*self.blocks)
        size = [max(xs) + 1, max(ys) + 1, max(zs) + 1]
        for x in range(size[0]):
            for z in range(size[2]):
                self.blocks.setdefault((x, 0, z), (FLOOR, None))

        palette, blocks = [], []
        for pos, (state, nbt) in sorted(self.blocks.items()):
            name, props = block_id(state)
            # Block states have used id/properties since data version 5006 (BlockStateFieldNamesFix).
            entry = {"id": name, "properties": props} if props else {"id": name}
            if entry not in palette:
                palette.append(entry)
            block = {"pos": list(pos), "state": palette.index(entry)}
            if nbt is not None:
                block["nbt"] = nbt
            blocks.append(block)

        structure = {"DataVersion": DATA_VERSION, "size": size, "palette": palette, "blocks": blocks, "entities": []}
        (data_dir / "structure").mkdir(parents=True, exist_ok=True)
        write_nbt(data_dir / "structure" / f"{self.name}.nbt", structure)

        for path, body in functions.items():
            target = data_dir / "function" / f"{path}.mcfunction"
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(body)

        max_ticks = max(self.max_ticks, self.check_delay * REPEATER_GAME_TICKS + 20)
        instance = {
            "type": "minecraft:block_based",
            "environment": "minecraft:default",
            "structure": f"{NAMESPACE}:{self.name}",
            "max_ticks": max_ticks,
        }
        (data_dir / "test_instance").mkdir(parents=True, exist_ok=True)
        (data_dir / "test_instance" / f"{self.name}.json").write_text(json.dumps(instance, indent=2) + "\n")


def load_tests():
    spec = importlib.util.spec_from_file_location("parity_tests", Path(__file__).with_name("tests.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.TESTS


def build(out):
    pack = out / "pumpkin-parity"
    if pack.exists():
        shutil.rmtree(pack)
    data_dir = pack / "data" / NAMESPACE
    data_dir.mkdir(parents=True)
    (pack / "pack.mcmeta").write_text(json.dumps(
        {"pack": {"description": "Pumpkin vanilla parity tests", "min_format": PACK_FORMAT, "max_format": PACK_FORMAT}},
        indent=2,
    ) + "\n")
    for test in load_tests():
        test.build(data_dir)
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, required=True, help="folder to write the pack into")
    args = parser.parse_args()
    build(args.out)
    print(f"Wrote {args.out / 'pumpkin-parity'}")


if __name__ == "__main__":
    main()
