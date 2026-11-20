#!/usr/bin/env python3
"""Writes the license texts of every Rust crate that is linked into the airplay binaries.

    python scripts/third-party-licenses.py [--target TRIPLE] > THIRD-PARTY-LICENSES.txt

Run it inside the MSYS2 UCRT64 shell (cargo and python from there). Only normal
dependencies count: build dependencies and tests are not shipped. The default target is
x86_64-pc-windows-gnu. Identical license texts are printed once, followed by the crates
that use them. No local paths are written to the output.
"""

import hashlib
import json
import pathlib
import subprocess
import sys

DEFAULT_TARGET = "x86_64-pc-windows-gnu"
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE")


def shipped_packages(target):
    # `cargo tree` resolves features the way the build does; `cargo metadata` would also
    # list optional crates that are never compiled. Metadata only supplies the paths.
    tree = subprocess.check_output(
        [
            "cargo", "tree", "--locked", "--edges", "normal", "--prefix", "none",
            "--format", "{p}", "--target", target,
        ],
        text=True,
    )
    wanted = set()
    for line in tree.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            wanted.add((parts[0], parts[1][1:]))
    meta = json.loads(
        subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"])
    )
    root = meta["resolve"]["root"]
    packages = [
        p
        for p in meta["packages"]
        if (p["name"], p["version"]) in wanted and p["id"] != root
    ]
    return sorted(packages, key=lambda p: (p["name"], p["version"]))


def is_license_name(name):
    upper = name.upper()
    if not upper.endswith((".TXT", ".MD", "")) and "." in upper:
        return False
    return upper.startswith(LICENSE_PREFIXES) or upper.endswith("-LICENSE.TXT")


def is_license_name_sub(name):
    # In subfolders also OFL/UFL texts (fonts) count.
    upper = name.upper()
    stem = upper.rsplit(".", 1)[0]
    return is_license_name(name) or stem in ("OFL", "UFL") or upper.startswith(("OFL-", "UFL-"))


SKIP_DIRS = {"src", "tests", "test", "benches", "examples", "target", ".git", "testdata", "test_data"}


def license_files(package):
    directory = pathlib.Path(package["manifest_path"]).parent
    files = [
        path
        for path in sorted(directory.iterdir())
        if path.is_file() and path.name.upper().startswith(LICENSE_PREFIXES)
    ]
    # Some crates keep licenses in subfolders (e.g. fonts/OFL.txt). Source code and test
    # data are skipped. Only used for crates without a license file in the top folder.
    if not files:
        for path in sorted(directory.rglob("*")):
            rel = path.relative_to(directory)
            if (
                path.is_file()
                and len(rel.parts) > 1
                and not (set(p.lower() for p in rel.parts[:-1]) & SKIP_DIRS)
                and is_license_name_sub(path.name)
            ):
                files.append(path)
    if package.get("license_file"):
        extra = directory / package["license_file"]
        if extra.is_file() and extra not in files:
            files.append(extra)
    return files


def main():
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    target = DEFAULT_TARGET
    if len(sys.argv) == 3 and sys.argv[1] == "--target":
        target = sys.argv[2]
    elif len(sys.argv) != 1:
        sys.exit(__doc__)

    texts = {}  # hash -> (text, [crate labels])
    missing = []
    packages = shipped_packages(target)
    for package in packages:
        label = f"{package['name']} {package['version']} ({package.get('license') or 'see text'})"
        files = license_files(package)
        if not files:
            missing.append((package, label))
        for path in files:
            text = path.read_text(encoding="utf-8", errors="replace").strip()
            key = hashlib.sha256(text.encode()).hexdigest()
            texts.setdefault(key, (text, []))[1].append(f"{label}, file {path.name}")

    out = sys.stdout
    out.write("THIRD-PARTY LICENSES\n\n")
    out.write(
        f"airplay includes the following Rust crates when built for {target}. "
        "Their license texts follow.\n"
        "airplay itself is licensed under GPL-3.0-only, see LICENSE. It is based on UxPlay "
        "(GPL-3.0), see licenses/UxPlay-GPL-3.0.txt in the distribution.\n"
        "The DLLs shipped next to airplay.exe come from MSYS2 and carry their own licenses, "
        "see licenses/dll/ in the distribution.\n\n"
    )
    for package in packages:
        out.write(f"  {package['name']} {package['version']}  {package.get('license') or ''}\n")
    for text, users in texts.values():
        out.write("\n" + "=" * 78 + "\n")
        out.write("Used by:\n")
        for user in users:
            out.write(f"  {user}\n")
        out.write("-" * 78 + "\n\n")
        out.write(text + "\n")
    for package, label in missing:
        out.write("\n" + "=" * 78 + "\n")
        out.write(f"Used by:\n  {label} (the crate ships no license file)\n")
        out.write("-" * 78 + "\n\n")
        if "MIT" in (package.get("license") or ""):
            authors = ", ".join(package.get("authors") or []) or f"the {package['name']} authors"
            out.write(MIT.format(authors=authors) + "\n")
        else:
            out.write(f"Licensed under {package.get('license')}.\n")
            print(f"warning: no license text for {label}", file=sys.stderr)


MIT = """MIT License

Copyright (c) {authors}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""


if __name__ == "__main__":
    main()
