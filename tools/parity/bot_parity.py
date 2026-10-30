#!/usr/bin/env python3
"""Run a Mineflayer bot check against vanilla and Pumpkin and diff what the bot saw.

Each check in tools/parity/bots/ is a Node script that connects to $PORT, may send console
commands by appending lines to the file in $CMDS, and prints `RESULT <key> <value>` lines. Lines that differ
between the servers are reported. Mineflayer comes from the 26.3 workspace at
https://github.com/ydw1904/mineflayer-26.3 (pass --mineflayer if it isn't in the cache folder).

    python tools/parity/bot_parity.py smoke
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import parity  # noqa: E402

BOTS = Path(__file__).with_name("bots")
VANILLA_READY = re.compile(r"Done \(")
PUMPKIN_READY = re.compile(r"Server is now running")
PUMPKIN_SETTINGS = {
    "online_mode": "false",
    "encryption": "false",
    "view_distance": "6",
    "simulation_distance": "6",
    "spawn_protection": "0",
    "allow_nether": "false",
    "allow_end": "false",
}


class Server:
    """A server process whose console reads lines appended to a command file."""

    def __init__(self, cmd, cwd, ready, log_path, cmds_path, env=None):
        self.cmds_path = cmds_path
        cmds_path.write_text("")
        self.log = open(log_path, "w", encoding="utf8", errors="replace")
        self.ready = threading.Event()
        self.proc = subprocess.Popen(cmd, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env,
                                     stderr=subprocess.STDOUT, text=True, encoding="utf8", errors="replace")
        threading.Thread(target=self._read, args=(ready,), daemon=True).start()
        threading.Thread(target=self._feed, daemon=True).start()

    def _read(self, ready):
        for line in self.proc.stdout:
            self.log.write(line)
            self.log.flush()
            if ready.search(line):
                self.ready.set()

    def _feed(self):
        sent = 0
        while self.proc.poll() is None:
            lines = self.cmds_path.read_text().splitlines()
            for line in lines[sent:]:
                self.send(line)
            sent = len(lines)
            time.sleep(0.1)

    def send(self, line):
        try:
            self.proc.stdin.write(line + "\n")
            self.proc.stdin.flush()
        except OSError:
            pass

    def stop(self, timeout=60):
        self.send("stop")
        try:
            self.proc.wait(timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()


def set_toml(text, key, value):
    return re.sub(rf"(?m)^{key} = .*$", f"{key} = {value}", text)


def start_pumpkin(binary, run_dir, port, seed, rng_seed=None):
    run_dir.mkdir(parents=True, exist_ok=True)
    config = run_dir / "pumpkin.toml"
    if not config.exists():
        # The first boot writes the default config; stop it and adjust the copy.
        first = Server([str(binary)], run_dir, PUMPKIN_READY, run_dir / "first-boot.log", run_dir / "cmds.txt")
        if not first.ready.wait(300):
            sys.exit(f"Pumpkin didn't start; see {run_dir / 'first-boot.log'}")
        first.stop()
        shutil.rmtree(run_dir / "world", ignore_errors=True)
    text = config.read_text()
    for key, value in {**PUMPKIN_SETTINGS, "seed": f'"{seed}"'}.items():
        text = set_toml(text, key, value)
    text = re.sub(r'(?m)^address = "0\.0\.0\.0:25565"$', f'address = "0.0.0.0:{port}"', text)
    text = re.sub(r"(?ms)(\[telemetry\]\s*enabled = )true", r"\1false", text)
    text = re.sub(r"(?ms)(\[networking\.bedrock\]\s*enabled = )true", r"\1false", text)
    config.write_text(text)
    return Server([str(binary)], run_dir, PUMPKIN_READY, run_dir / "server.log", run_dir / "cmds.txt",
                  parity.pumpkin_rng_env(rng_seed, run_dir / "rng.log"))


def start_vanilla(java, jar, run_dir, port, seed, cache, rng_seed=None):
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "eula.txt").write_text("eula=true\n")
    (run_dir / "server.properties").write_text("\n".join([
        "online-mode=false",
        "white-list=false",
        "enforce-whitelist=false",
        f"server-port={port}",
        f"level-seed={seed}",
        "view-distance=6",
        "simulation-distance=6",
        "spawn-protection=0",
        "enable-command-block=true",
        "sync-chunk-writes=false",
    ]) + "\n")
    cmd = parity.vanilla_command(java, jar, cache, "net.minecraft.server.Main", rng_seed, run_dir / "rng.log")
    return Server(cmd + ["nogui"], run_dir, VANILLA_READY, run_dir / "server.log", run_dir / "cmds.txt")


def run_check(server, check, port, mineflayer, out_path, timeout):
    if not server.ready.wait(600):
        return None
    env = {**os.environ, "PORT": str(port), "CMDS": str(server.cmds_path),
           "NODE_PATH": str(mineflayer / "node_modules")}
    try:
        result = subprocess.run(["node", str(check)], cwd=mineflayer, env=env, capture_output=True,
                                text=True, encoding="utf8", errors="replace", timeout=timeout)
        output = result.stdout + result.stderr
    except subprocess.TimeoutExpired as err:
        output = (err.stdout or "") + f"\nTIMEOUT after {timeout}s\n"
    out_path.write_text(output, encoding="utf8")
    return output


CHECK_LINE = re.compile(r"^(PASS|FAIL) (.+?): got (.*?)(?: expected .*)?$")
INFO_LINE = re.compile(r"^INFO (.+?): (.*)$")
# Keys printed by steptrace.js checks: t<tick>.<probe>.
TRACE_KEY = re.compile(r"^t(\d+)\.")


def results(output):
    """Collects what a check observed, in the order it printed it.

    Besides `RESULT <key> <value>`, this reads the `PASS|FAIL <label>: got <value>` and
    `INFO <label>: <value>` lines of the pumpkin-checks in the mineflayer-26.3 workspace. For
    those only the observed value is compared, so vanilla stays the reference rather than the
    expectation written into the check.
    """
    found = {}
    for line in (output or "").splitlines():
        if line.startswith("RESULT "):
            key, _, value = line[len("RESULT "):].partition(" ")
            found[key] = value.strip()
        elif match := CHECK_LINE.match(line):
            found[match.group(2)] = match.group(3)
        elif match := INFO_LINE.match(line):
            found[f"info: {match.group(1)}"] = match.group(2)
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("check", help="check name in tools/parity/bots without .js, or a path inside "
                        "the workspace's pumpkin-checks such as falling/landing-sand.js")
    parser.add_argument("--cache", type=Path, default=parity.DEFAULT_CACHE)
    parser.add_argument("--mineflayer", type=Path, help="mineflayer-26.3 workspace (default: <cache>/mineflayer-26.3)")
    parser.add_argument("--seed", default="1792386707805314503")
    parser.add_argument("--java", help="java executable to run vanilla with")
    parser.add_argument("--timeout", type=int, default=300, help="seconds a check may run")
    parser.add_argument("--no-build", action="store_true", help="skip cargo build of pumpkin")
    parser.add_argument("--release", action="store_true", help="use a release build of pumpkin")
    parser.add_argument("--diffs-only", action="store_true", help="only print the lines that differ")
    parser.add_argument("--rng-seed", type=int,
                        help="lock gameplay randomness to per-owner streams from this seed and diff the draw logs")
    args = parser.parse_args()

    mineflayer = (args.mineflayer or args.cache / "mineflayer-26.3").resolve()
    # A bare name is one of ours; a path like `falling/landing-sand.js` is one of the workspace's
    # pumpkin-checks, run unchanged.
    check = BOTS / f"{args.check}.js" if "/" not in args.check else mineflayer / "pumpkin-checks" / args.check
    if not check.exists():
        sys.exit(f"No bot check {check}")
    if not (mineflayer / "node_modules" / "mineflayer").exists():
        sys.exit(f"No installed mineflayer-26.3 workspace at {mineflayer}")

    version = parity.target_version()
    jar, java_major = parity.ensure_server_jar(version, args.cache)
    java = parity.find_java(java_major, args.java)
    profile = "release" if args.release else "debug"
    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "pumpkin"] + (["--release"] if args.release else []),
                       cwd=parity.REPO, check=True)
    binary = parity.REPO / "target" / profile / ("pumpkin.exe" if os.name == "nt" else "pumpkin")

    run_dir = args.cache / "bot-runs" / f"{time.strftime('%Y%m%d-%H%M%S')}-{check.stem}"
    outputs = {}
    for name, start, port in (
        ("vanilla", lambda d, p: start_vanilla(java, jar, d, p, args.seed, args.cache, args.rng_seed), 25566),
        ("pumpkin", lambda d, p: start_pumpkin(binary, d, p, args.seed, args.rng_seed), 25565),
    ):
        print(f"Running {args.check} on {name}...")
        server = start(run_dir / name, port)
        try:
            outputs[name] = run_check(server, check, port, mineflayer, run_dir / f"{name}.out", args.timeout)
        finally:
            server.stop()
        if outputs[name] is None:
            print(f"  {name} didn't start; see {run_dir / name / 'server.log'}")

    vanilla, pumpkin = results(outputs["vanilla"]), results(outputs["pumpkin"])
    mismatches = 0
    first_tick = None
    for key in list(vanilla) + [k for k in pumpkin if k not in vanilla]:
        v, p = vanilla.get(key, "<missing>"), pumpkin.get(key, "<missing>")
        same = v == p
        mismatches += not same
        if not same and first_tick is None and (tick := TRACE_KEY.match(key)):
            first_tick = int(tick.group(1))
        if not (same and args.diffs_only):
            print(f"{'ok ' if same else 'DIFF'} {key}: vanilla={v}" + ("" if same else f"  pumpkin={p}"))
    if first_tick is not None:
        print(f"\nTraces first differ at tick {first_tick}.")
    if args.rng_seed is not None:
        print()
        parity.rng_diff.report(run_dir / "vanilla" / "rng.log", run_dir / "pumpkin" / "rng.log")
    print(f"\n{mismatches} differences. Logs in {run_dir}")
    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
