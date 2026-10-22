#!/usr/bin/env python3
"""Check and scrape the changelog notices of PR descriptions.

A notice is one fenced ```changelog block in the PR description (see
CONTRIBUTING.md). PRs without one have nothing for the changelog.

  python3 ci/changelog.py check < body.md
      Check the notice in a PR description read from stdin.
  python3 ci/changelog.py scrape <rev-range> [<crate>]
      Collect the notices of the PRs merged in <rev-range> (e.g.
      `chain-0.23.2..master`) into keepachangelog sections, one per crate.
      Needs `gh`.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

HEADINGS = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]
CRATES = sorted("bdk_" + p.name for p in (Path(__file__).parent.parent / "crates").iterdir() if p.is_dir())
BLOCK = re.compile(r"^```changelog[ \t]*\n(.*?)^```[ \t]*$", re.M | re.S)
ENTRY = re.compile(r"- ((?:`?bdk_\w+`?, )*`?bdk_\w+`?): (\S.*)")


def parse(body):
    """Return ([(heading, crates, text)], [error]) for a PR description."""
    body = re.sub(r"<!--.*?-->", "", body.replace("\r\n", "\n"), flags=re.S)
    blocks = BLOCK.findall(body)
    if len(blocks) > 1:
        return [], ["more than one ```changelog block"]
    if not blocks and re.search(rf"^### ({'|'.join(HEADINGS)})\b", body, re.M):
        return [], ["changelog headings outside a ```changelog block"]
    entries, errors, heading = [], [], None
    for n, line in enumerate(blocks[0].splitlines() if blocks else [], 1):
        if not line.strip():
            continue
        if line.startswith("### "):
            heading = line[4:].strip()
            if heading not in HEADINGS:
                errors.append(f"line {n}: heading must be one of {', '.join(HEADINGS)}: {line!r}")
            continue
        m = ENTRY.fullmatch(line.rstrip())
        if not m:
            errors.append(f"line {n}: expected `- <crate>: <change>` on one line: {line!r}")
            continue
        if heading is None:
            errors.append(f"line {n}: entry before any `### <heading>`")
            continue
        crates = m[1].replace("`", "").split(", ")
        for c in crates:
            if c not in CRATES:
                errors.append(f"line {n}: unknown crate {c!r} (one of {', '.join(CRATES)})")
        entries.append((heading, crates, m[2]))
    if blocks and not entries and not errors:
        errors.append("the ```changelog block has no entries (remove it if there is nothing to log)")
    return entries, errors


def check():
    entries, errors = parse(sys.stdin.read())
    for e in errors:
        print(f"error: {e}", file=sys.stderr)
    if not errors:
        print(f"ok: {len(entries)} changelog entries")
    return 1 if errors else 0


def scrape(rev_range, crate=None):
    subjects = subprocess.run(
        ["git", "log", "--merges", "--reverse", "--format=%s", rev_range],
        check=True, capture_output=True, text=True,
    ).stdout
    out = {}  # crate -> heading -> [line]
    for pr in re.findall(r"^Merge bitcoindevkit/bdk#(\d+):", subjects, re.M):
        body = json.loads(subprocess.run(
            ["gh", "pr", "view", pr, "--repo", "bitcoindevkit/bdk", "--json", "body"],
            check=True, capture_output=True, text=True,
        ).stdout)["body"]
        entries, errors = parse(body)
        for e in errors:
            print(f"warning: #{pr}: {e}", file=sys.stderr)
        for heading, crates, text in entries:
            if not text.endswith(f"#{pr}"):
                text += f" #{pr}"
            for c in crates:
                out.setdefault(c, {}).setdefault(heading, []).append(f"- {text}")
    for c in sorted(out):
        if crate and c != crate:
            continue
        print(f"## {c}\n")
        for h in HEADINGS:
            if h in out[c]:
                print(f"### {h}\n")
                print("\n".join(out[c][h]) + "\n")
    return 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["check"] and len(sys.argv) == 2:
        sys.exit(check())
    if sys.argv[1:2] == ["scrape"] and len(sys.argv) in (3, 4):
        sys.exit(scrape(*sys.argv[2:]))
    sys.exit(__doc__)
