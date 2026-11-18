"""Writes THIRD-PARTY-LICENSES.txt for the web version: every library the
browser builds are made from, with its licence and the licence texts it
ships, and the pictures and fonts that are someone else's work.

    python web\\licences.py <output file>

build.ps1 runs it into the output folder after the builds. It reads
`cargo metadata` for the browser target with every feature on (so the
many-threads build's libraries are in too), walks what golem_web really
depends on from the resolved graph, and leaves out the project's own crates
(those that live in this repository). A licence text that several libraries
ship word for word is printed once, with all their names over it.
"""

import json
import os
import subprocess
import sys

WEB = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(WEB)

# Files in a library's folder that hold its licence or notices.
LICENCE_NAMES = ("license", "licence", "copying", "notice", "copyright", "ufl", "unlicense")


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--all-features",
         "--filter-platform", "wasm32-unknown-unknown"],
        cwd=WEB, check=True, capture_output=True,
    ).stdout
    return json.loads(out)


def used_packages(meta):
    """Every package golem_web pulls in for the browser, by id."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            # Only what is built into the program, not build scripts' or tests' helpers.
            kinds = {k["kind"] for k in dep["dep_kinds"]}
            if kinds & {None, "normal"} or None in kinds:
                todo.append(dep["pkg"])
    seen.discard(root)
    return seen


def is_ours(pkg):
    folder = os.path.dirname(pkg["manifest_path"])
    try:
        inside = os.path.commonpath([os.path.abspath(folder), REPO]) == REPO
    except ValueError:
        inside = False
    # The slimmed font crate is egui's, kept here only to leave two fonts out.
    return inside and pkg["name"] != "epaint_default_fonts"


def licence_files(folder):
    found = []
    for root, dirs, files in os.walk(folder):
        # A library's own licence is at its top, or one folder down (fonts/, LICENSES/).
        depth = os.path.relpath(root, folder).count(os.sep) + (0 if root == folder else 1)
        if depth > 1:
            dirs[:] = []
            continue
        dirs[:] = [d for d in dirs if d.lower() not in ("target", ".git", "tests", "benches", "examples", "src")
                   or d.lower() in ("licenses", "licences", "fonts")]
        for name in sorted(files):
            low = name.lower()
            if low.startswith(LICENCE_NAMES) or (low.endswith(".txt") and ("license" in low or "licence" in low)):
                found.append(os.path.join(root, name))
    return found


def read_text(path):
    with open(path, "rb") as f:
        data = f.read()
    return data.decode("utf-8", errors="replace").replace("\r\n", "\n").strip()


ASSETS = """\
Pictures and fonts that are not GolemSculpt's own
--------------------------------------------------

Matcaps "Clay 1" to "Clay 7" (assets/matcaps/Ben Simonds)
    Created by Ben Simonds, licensed under a Creative Commons Attribution 3.0
    Unported License: http://creativecommons.org/licenses/by/3.0/
    Based on a work at bensimonds.com
    Original source: http://bensimonds.com/2010/07/30/matcap-generator/

Fonts (egui's default fonts, carried in the epaint_default_fonts library below)
    Ubuntu Light, under the Ubuntu Font Licence 1.0.
    emoji-icon-font, under the MIT licence.
    Their licence texts are printed with epaint_default_fonts.

Fonts for Japanese, Chinese and Korean tooltips (fonts/cjk-*.otf)
    Noto Sans CJK DemiLight, Copyright 2014-2021 Adobe (http://www.adobe.com/),
    cut down to the letters the tooltips use (web/fonts/subset_cjk.py).
    Under the SIL Open Font License 1.1, whose text follows.
"""


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    meta = metadata()
    packages = {p["id"]: p for p in meta["packages"]}
    used = [packages[i] for i in used_packages(meta)]
    theirs = sorted((p for p in used if not is_ours(p)), key=lambda p: (p["name"].lower(), p["version"]))

    texts = {}  # licence text -> names of the libraries that ship it
    rows = []
    missing = []
    for p in theirs:
        label = f'{p["name"]} {p["version"]}'
        licence = p.get("license") or (f'see {p["license_file"]}' if p.get("license_file") else "not stated")
        where = p.get("repository") or p.get("homepage") or ""
        rows.append((label, licence, where))
        files = licence_files(os.path.dirname(p["manifest_path"]))
        for path in files:
            texts.setdefault(read_text(path), []).append(f'{label} ({os.path.basename(path)})')
        if not files:
            missing.append(label)

    out = []
    out.append("GolemSculpt for the web: third-party licences")
    out.append("=============================================")
    out.append("")
    out.append("GolemSculpt is built with the open-source libraries listed below, and uses")
    out.append("the pictures and fonts named here. Each is the work of its authors and is")
    out.append("used under its own licence, whose text follows the list.")
    out.append("")
    out.append(ASSETS)
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "fonts", "OFL.txt"), encoding="utf-8") as f:
        out.append(f.read().strip())
        out.append("")
    out.append("Libraries")
    out.append("---------")
    out.append("")
    width = max(len(r[0]) for r in rows)
    for label, licence, where in rows:
        out.append(f"{label.ljust(width)}  {licence}" + (f"  {where}" if where else ""))
    if missing:
        out.append("")
        out.append("These ship no licence file of their own. Their licence is the one named")
        out.append("above (its standard text is printed below under another library's name),")
        out.append("and their copyright belongs to the authors named at the repository given:")
        out.append("    " + ", ".join(missing))
    out.append("")
    out.append("")
    out.append("Licence texts")
    out.append("-------------")
    for text, names in sorted(texts.items(), key=lambda kv: kv[1][0].lower()):
        out.append("")
        out.append("=" * 78)
        for name in names:
            out.append(name)
        out.append("=" * 78)
        out.append("")
        out.append(text)
    with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print(f"{sys.argv[1]}: {len(rows)} libraries, {len(texts)} licence texts"
          + (f", {len(missing)} without a file of their own" if missing else ""))


if __name__ == "__main__":
    main()
