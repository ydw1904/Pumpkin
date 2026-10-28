#!/usr/bin/env python3
"""Run the same datapack GameTests on vanilla and Pumpkin and compare the results.

Vanilla runs through its headless runner (net.minecraft.gametest.Main), Pumpkin through
`pumpkin-tester parity`. Both write JUnit-like XML, which this script diffs.

    python tools/parity/parity.py --packs assets/tests/datapacks --tests "pumpkin:*"
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
MANIFEST_URL = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
DEFAULT_CACHE = Path.home() / "pumpkin-parity"


def target_version():
    text = (REPO / "crates/pumpkin-data/src/generated/packet.rs").read_text()
    match = re.search(r"CURRENT_MC_VERSION: JavaMinecraftVersion =\s*\S*::V_(\w+);", text)
    if not match:
        sys.exit("Could not read CURRENT_MC_VERSION from pumpkin-data")
    return match.group(1).replace("_", ".")


def fetch_json(url):
    with urllib.request.urlopen(url) as response:
        return json.load(response)


def ensure_server_jar(version, cache):
    """Downloads the vanilla server jar for `version` once and returns (jar, java major)."""
    folder = cache / f"vanilla-{version}"
    jar = folder / "server.jar"
    meta_file = folder / "version.json"
    if not meta_file.exists():
        manifest = fetch_json(MANIFEST_URL)
        entry = next((v for v in manifest["versions"] if v["id"] == version), None)
        if entry is None:
            sys.exit(f"Version {version} is not in the Mojang manifest")
        folder.mkdir(parents=True, exist_ok=True)
        meta_file.write_text(json.dumps(fetch_json(entry["url"])))
    meta = json.loads(meta_file.read_text())
    server = meta["downloads"]["server"]
    if not jar.exists() or hashlib.sha1(jar.read_bytes()).hexdigest() != server["sha1"]:
        print(f"Downloading vanilla {version} server...")
        urllib.request.urlretrieve(server["url"], jar)
        if hashlib.sha1(jar.read_bytes()).hexdigest() != server["sha1"]:
            sys.exit("Downloaded server.jar has the wrong SHA-1")
    return jar, meta["javaVersion"]["majorVersion"]


def find_java(major, explicit):
    candidates = [explicit] if explicit else []
    if os.environ.get("JAVA_HOME"):
        candidates.append(str(Path(os.environ["JAVA_HOME"]) / "bin" / "java"))
    for root in ("C:/Program Files/Eclipse Adoptium", "C:/Program Files/Java", "/usr/lib/jvm"):
        if Path(root).is_dir():
            candidates += [str(p / "bin" / "java") for p in sorted(Path(root).iterdir(), reverse=True)]
    candidates.append("java")
    for java in candidates:
        try:
            out = subprocess.run([java, "-version"], capture_output=True, text=True).stderr
        except OSError:
            continue
        found = re.search(r'version "(\d+)', out)
        if found and int(found.group(1)) >= major:
            return java
    sys.exit(f"No Java {major}+ found; pass --java or set JAVA_HOME")


def run_vanilla(java, jar, packs, tests, run_dir):
    run_dir.mkdir(parents=True, exist_ok=True)
    shutil.copytree(packs, run_dir / "packs")
    (run_dir / "eula.txt").write_text("eula=true\n")
    report = run_dir / "report.xml"
    cmd = [java, "-Xmx2G", "-DbundlerMainClass=net.minecraft.gametest.Main", "-jar", str(jar),
           "--packs", "packs", "--tests", tests, "--report", str(report)]
    with open(run_dir / "run.log", "w") as log:
        subprocess.run(cmd, cwd=run_dir, stdout=log, stderr=subprocess.STDOUT)
    if not report.exists():
        sys.exit(f"Vanilla produced no report; see {run_dir / 'run.log'}")
    return report, run_dir / "gametestserver" / "gametestworld"


def run_pumpkin(tester, packs, tests, template, run_dir):
    run_dir.mkdir(parents=True, exist_ok=True)
    report = run_dir / "report.xml"
    cmd = [str(tester), "parity", "--packs", str(packs), "--tests", tests,
           "--report", str(report), "--world-template", str(template)]
    with open(run_dir / "run.log", "w") as log:
        subprocess.run(cmd, cwd=run_dir, stdout=log, stderr=subprocess.STDOUT)
    if not report.exists():
        sys.exit(f"Pumpkin produced no report; see {run_dir / 'run.log'}")
    return report


def read_report(path):
    results = {}
    for case in ET.parse(path).iter("testcase"):
        failure = case.find("failure")
        if failure is None:
            failure = case.find("skipped")
        results[case.get("name")] = None if failure is None else failure.get("message", "failed")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--packs", type=Path, default=REPO / "assets/tests/datapacks",
                        help="folder of datapacks containing test_instance definitions")
    parser.add_argument("--tests", default="*", help="vanilla test selector, e.g. 'pumpkin:*'")
    parser.add_argument("--cache", type=Path, default=DEFAULT_CACHE, help="where jars and runs are kept")
    parser.add_argument("--java", help="java executable to run vanilla with")
    parser.add_argument("--no-build", action="store_true", help="skip cargo build of pumpkin-tester")
    args = parser.parse_args()

    version = target_version()
    jar, java_major = ensure_server_jar(version, args.cache)
    java = find_java(java_major, args.java)

    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "pumpkin-tester"], cwd=REPO, check=True)
    tester = REPO / "target" / "debug" / ("pumpkin-tester.exe" if os.name == "nt" else "pumpkin-tester")

    run_dir = args.cache / "runs" / time.strftime("%Y%m%d-%H%M%S")
    print(f"Running vanilla {version}...")
    vanilla_report, template = run_vanilla(java, jar, args.packs.resolve(), args.tests, run_dir / "vanilla")
    print("Running Pumpkin...")
    pumpkin_report = run_pumpkin(tester, args.packs.resolve(), args.tests, template, run_dir / "pumpkin")

    vanilla = read_report(vanilla_report)
    pumpkin = read_report(pumpkin_report)
    mismatches = 0
    rows = []
    for test in sorted(set(vanilla) | set(pumpkin)):
        v, p = vanilla.get(test, "missing"), pumpkin.get(test, "missing")
        if v is None and p is None:
            status = "ok"
        elif v is None:
            status, mismatches = "PARITY", mismatches + 1
        elif p is None:
            status, mismatches = "PUMPKIN-ONLY PASS", mismatches + 1
        else:
            status = "both fail (test broken?)"
        rows.append((status, test, v, p))

    for status, test, v, p in rows:
        print(f"{status:<26} {test}")
        if v is not None:
            print(f"{'':<26}   vanilla: {v}")
        if p is not None:
            print(f"{'':<26}   pumpkin: {p}")
    print(f"\n{len(rows)} tests, {mismatches} mismatches. Logs in {run_dir}")
    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
