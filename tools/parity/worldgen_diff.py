#!/usr/bin/env python3
"""Generate the same seed on vanilla and Pumpkin and diff the worlds block by block.

Both servers force-load a square of chunks, save, and stop. Pumpkin saves in Anvil (zlib) for
this run so one reader handles both worlds. Only chunks both servers saved as fully generated
are compared. Differences are grouped by (vanilla block, Pumpkin block) with sample positions,
and by chunk.

    python tools/parity/worldgen_diff.py --seed 1792386707805314503 --center -2,0 --radius 4
"""

import argparse
import collections
import gzip
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import bot_parity  # noqa: E402
import parity  # noqa: E402

# How many example positions to print per kind of difference.
SAMPLES = 5
# How many kinds of difference and chunks to list.
TOP = 40


def read_nbt(data):
    """Parses an uncompressed NBT compound into Python values (lists, dicts, ints, strings)."""
    pos = 0

    def take(fmt):
        nonlocal pos
        value = struct.unpack_from(fmt, data, pos)
        pos += struct.calcsize(fmt)
        return value[0]

    def string():
        nonlocal pos
        length = take(">H")
        value = data[pos:pos + length].decode("utf8", "replace")
        pos += length
        return value

    def payload(tag):
        nonlocal pos
        if tag == 1:
            return take(">b")
        if tag == 2:
            return take(">h")
        if tag == 3:
            return take(">i")
        if tag == 4:
            return take(">q")
        if tag == 5:
            return take(">f")
        if tag == 6:
            return take(">d")
        if tag == 7:
            length = take(">i")
            value = data[pos:pos + length]
            pos += length
            return value
        if tag == 8:
            return string()
        if tag == 9:
            item, length = take(">b"), take(">i")
            return [payload(item) for _ in range(length)]
        if tag == 10:
            compound = {}
            while (item := take(">b")) != 0:
                name = string()
                compound[name] = payload(item)
            return compound
        if tag == 11:
            length = take(">i")
            value = struct.unpack_from(f">{length}i", data, pos)
            pos += 4 * length
            return value
        if tag == 12:
            length = take(">i")
            value = struct.unpack_from(f">{length}q", data, pos)
            pos += 8 * length
            return value
        raise ValueError(f"unknown NBT tag {tag}")

    if take(">b") != 10:
        raise ValueError("chunk NBT isn't a compound")
    string()
    return payload(10)


def read_region(path):
    """Yields (chunk x, chunk z, NBT) for every chunk in an Anvil region file."""
    data = path.read_bytes()
    for index in range(1024):
        location = struct.unpack_from(">I", data, index * 4)[0]
        offset = (location >> 8) * 4096
        if offset == 0:
            continue
        length, compression = struct.unpack_from(">Ib", data, offset)
        body = data[offset + 5:offset + 4 + length]
        if compression == 1:
            raw = gzip.decompress(body)
        elif compression == 2:
            raw = zlib.decompress(body)
        elif compression == 3:
            raw = body
        else:
            sys.exit(f"{path}: chunk compression {compression} isn't supported")
        nbt = read_nbt(raw)
        yield nbt["xPos"], nbt["zPos"], nbt


# Default properties per block, from vanilla's block report; see `load_defaults`.
DEFAULTS = {}


def load_defaults(java, jar, cache, version):
    """Fills DEFAULTS from vanilla's `--reports` block list, generating it once into the cache."""
    out = cache / f"reports-{version}"
    report = out / "reports" / "blocks.json"
    if not report.exists():
        subprocess.run([java, "-DbundlerMainClass=net.minecraft.data.Main", "-jar", str(jar),
                        "--reports", "--output", str(out)], cwd=cache, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for name, block in json.loads(report.read_text(encoding="utf8")).items():
        default = next(state for state in block["states"] if state.get("default"))
        DEFAULTS[name.removeprefix("minecraft:")] = default.get("properties", {})


def state_name(entry):
    """`name[key=value,...]` with every property spelled out, sorted by key."""
    if isinstance(entry, dict) and set(entry) == {""}:
        # A list mixing tag types wraps each element as {"": value}.
        entry = entry[""]
    if isinstance(entry, str):
        # 26.x writes a block's default state as its bare id.
        name, props = entry, {}
    elif "id" in entry:
        # ...and other states as {id, properties}, listing only what differs from the default.
        name, props = entry["id"], entry.get("properties", {})
    else:
        name, props = entry["Name"], entry.get("Properties", {})
    name = name.removeprefix("minecraft:")
    props = {**DEFAULTS.get(name, {}), **props}
    if not props:
        return name
    return name + "[" + ",".join(f"{k}={v}" for k, v in sorted(props.items())) + "]"


def section_blocks(section):
    """The 4096 block state names of a section, indexed y * 256 + z * 16 + x."""
    states = section.get("block_states")
    if not states:
        return None
    palette = [state_name(entry) for entry in states["palette"]]
    if len(palette) == 1:
        return [palette[0]] * 4096
    bits = max(4, (len(palette) - 1).bit_length())
    per_long = 64 // bits
    mask = (1 << bits) - 1
    blocks = []
    for value in states["data"]:
        value &= (1 << 64) - 1
        for i in range(per_long):
            if len(blocks) == 4096:
                break
            blocks.append(palette[(value >> (i * bits)) & mask])
    return blocks


def load_world(world):
    """Fully generated chunks of the overworld, as {(cx, cz): {section y: blocks}}."""
    region_dirs = [world / "region", world / "dimensions" / "minecraft" / "overworld" / "region"]
    chunks = {}
    for region_dir in region_dirs:
        for path in sorted(region_dir.glob("r.*.mca")) if region_dir.exists() else []:
            for cx, cz, nbt in read_region(path):
                if nbt.get("Status", "").removeprefix("minecraft:") != "full":
                    continue
                chunks[(cx, cz)] = {s["Y"]: section_blocks(s) for s in nbt.get("sections", [])}
    return chunks


def generate(name, start, run_dir, area, wait):
    """Starts a server, force-loads `area` (x1, z1, x2, z2 in blocks), waits, saves and stops."""
    server = start(run_dir)
    if not server.ready.wait(600):
        server.stop()
        sys.exit(f"{name} didn't start; see {run_dir / 'server.log'}")
    server.send("forceload add {} {} {} {}".format(*area))
    print(f"  {name}: waiting {wait}s for generation...")
    time.sleep(wait)
    server.send("save-all flush")
    time.sleep(10)
    server.stop()


def pumpkin_anvil_config(run_dir):
    config = run_dir / "pumpkin.toml"
    text = config.read_text()
    text = re.sub(r'(?m)^\[world\.chunk\]\ntype = "pump"$',
                  '[world.chunk]\ntype = "anvil"\n\n[world.chunk.compression]\nalgorithm = "ZLib"\nlevel = 6',
                  text)
    config.write_text(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--seed", default="1792386707805314503")
    parser.add_argument("--center", default="0,0", help="center chunk as x,z")
    parser.add_argument("--radius", type=int, default=4, help="chunks on each side of the center")
    parser.add_argument("--wait", type=int, default=90, help="seconds to let each server generate")
    parser.add_argument("--cache", type=Path, default=parity.DEFAULT_CACHE)
    parser.add_argument("--java", help="java executable to run vanilla with")
    parser.add_argument("--no-build", action="store_true", help="skip cargo build of pumpkin")
    parser.add_argument("--worlds", type=Path, help="compare an earlier run's folder instead of generating")
    args = parser.parse_args()

    cx, cz = (int(v) for v in args.center.split(","))
    r = args.radius
    area = ((cx - r) * 16, (cz - r) * 16, (cx + r) * 16 + 15, (cz + r) * 16 + 15)
    wanted = {(x, z) for x in range(cx - r, cx + r + 1) for z in range(cz - r, cz + r + 1)}

    version = parity.target_version()
    jar, java_major = parity.ensure_server_jar(version, args.cache)
    java = parity.find_java(java_major, args.java)
    load_defaults(java, jar, args.cache, version)
    if args.worlds:
        run_dir = args.worlds
    else:
        if not args.no_build:
            subprocess.run(["cargo", "build", "-p", "pumpkin"], cwd=parity.REPO, check=True)
        binary = parity.REPO / "target" / "debug" / ("pumpkin.exe" if os.name == "nt" else "pumpkin")
        run_dir = args.cache / "worldgen-runs" / f"{time.strftime('%Y%m%d-%H%M%S')}-{args.seed}"

        print("Generating on vanilla...")
        generate("vanilla", lambda d: bot_parity.start_vanilla(java, jar, d, 25566, args.seed, args.cache),
                 run_dir / "vanilla", area, args.wait)

        def start_pumpkin(d):
            # The first call writes the config; switch it to Anvil before the real start.
            bot_parity.start_pumpkin(binary, d, 25565, args.seed).stop()
            pumpkin_anvil_config(d)
            shutil.rmtree(d / "world", ignore_errors=True)
            return bot_parity.start_pumpkin(binary, d, 25565, args.seed)

        print("Generating on Pumpkin...")
        generate("pumpkin", start_pumpkin, run_dir / "pumpkin", area, args.wait)

    vanilla = load_world(run_dir / "vanilla" / "world")
    pumpkin = load_world(run_dir / "pumpkin" / "world")
    common = sorted(wanted & set(vanilla) & set(pumpkin))
    print(f"\nChunks: {len(wanted)} wanted, vanilla saved {len(wanted & set(vanilla))}, "
          f"Pumpkin saved {len(wanted & set(pumpkin))}, comparing {len(common)}")

    kinds = collections.Counter()
    samples = collections.defaultdict(list)
    per_chunk = collections.Counter()
    compared = 0
    for chunk in common:
        v_sections, p_sections = vanilla[chunk], pumpkin[chunk]
        for y in sorted(set(v_sections) | set(p_sections)):
            v_blocks, p_blocks = v_sections.get(y), p_sections.get(y)
            if v_blocks is None and p_blocks is None:
                continue
            v_blocks = v_blocks or ["air"] * 4096
            p_blocks = p_blocks or ["air"] * 4096
            compared += 4096
            if v_blocks == p_blocks:
                continue
            for i, (v, p) in enumerate(zip(v_blocks, p_blocks)):
                if v == p:
                    continue
                kind = (v, p)
                kinds[kind] += 1
                per_chunk[chunk] += 1
                if len(samples[kind]) < SAMPLES:
                    x = chunk[0] * 16 + (i & 15)
                    z = chunk[1] * 16 + (i >> 4 & 15)
                    samples[kind].append(f"{x},{y * 16 + (i >> 8)},{z}")

    total = sum(kinds.values())
    print(f"{total} of {compared} blocks differ ({100 * total / max(compared, 1):.4f}%)")
    names = collections.Counter()
    for (v, p), n in kinds.items():
        names[(v.split("[")[0], p.split("[")[0])] += n
    print("\nBy block (vanilla -> Pumpkin), properties ignored:")
    for (v, p), n in names.most_common(TOP):
        print(f"  {n:8} {v} -> {p}")
    print("\nBy state, with sample positions:")
    for kind, n in kinds.most_common(TOP):
        print(f"  {n:8} {kind[0]} -> {kind[1]}   e.g. {' '.join(samples[kind])}")
    print("\nBy chunk:")
    for chunk, n in per_chunk.most_common(TOP):
        print(f"  {n:8} chunk {chunk[0]},{chunk[1]}")
    print(f"\nWorlds in {run_dir}")


if __name__ == "__main__":
    main()
