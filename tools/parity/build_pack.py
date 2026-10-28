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

    def check_after(self, repeaters, *conditions):
        """Checks `conditions` (pos, state, description) once `repeaters` delay-4 repeaters have
        carried the start signal along x=0 to a command block."""
        self.check_delay = repeaters
        self.checks = list(conditions)
        return self

    def _build_checks(self):
        if not self.checks:
            return None
        k = self.check_delay
        for z in range(1, k + 1):
            # A diode's facing points at its input, so these carry the signal towards +z.
            self.set((0, 1, z), "minecraft:repeater[delay=4,facing=north,locked=false,powered=false]")
        cmd = (0, 1, k + 1)
        self.command(cmd, f"function {NAMESPACE}:check/{self.name}")
        self.accept((0, 3, k + 1))
        lines = []
        for i, (pos, state, description) in enumerate(self.checks):
            self.fail((0, 3, k + 2 + i), f"expected {description}")
            rel = "~{} ~{} ~{}".format(pos[0] - cmd[0], pos[1] - cmd[1], pos[2] - cmd[2])
            lines.append(f"execute unless block {rel} {state} run setblock ~ ~1 ~{1 + i} minecraft:redstone_block")
        conditions = " ".join(
            "if block ~{} ~{} ~{} {}".format(p[0] - cmd[0], p[1] - cmd[1], p[2] - cmd[2], s)
            for p, s, _ in self.checks
        )
        lines.append(f"execute {conditions} run setblock ~ ~1 ~ minecraft:redstone_block")
        return "\n".join(lines) + "\n"

    def build(self, data_dir):
        function = self._build_checks()
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

        if function:
            (data_dir / "function" / "check").mkdir(parents=True, exist_ok=True)
            (data_dir / "function" / "check" / f"{self.name}.mcfunction").write_text(function)

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
