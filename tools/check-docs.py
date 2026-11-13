#!/usr/bin/env python3
"""Check the documents against the tree they describe.

    python3 tools/check-docs.py            # every tracked markdown file
    python3 tools/check-docs.py README.md  # one file

A README rots quietly: a link to a file that moved, an anchor to a heading
that was renamed, a command whose flag no longer exists, an image heavier than
a page should carry. None of those fail a build, so this checks them the way a
reader would follow them:

- every relative link and image resolves inside the repository;
- every in-page anchor names a heading that exists (GitHub's slug rules);
- every `cargo run`/`tools/*.sh`/`society` command in a bash block names a
  binary that exists and flags the binary actually accepts;
- images stay under the size a README should ship;
- headings have one H1 and no skipped levels, and code fences balance.

Exit status is 1 when anything is wrong, so it can gate a change.
"""
from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IMAGE_LIMIT = 4_000_000  # a README clip heavier than this is a slow page
FENCE = re.compile(r"^```(\w*)\s*$")
LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
IMAGE = re.compile(r"!\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*$")
INLINE_CODE = re.compile(r"`([^`]+)`")
SHELL_TOKEN = re.compile(r"(?:^|[\s;|&(])(cargo|tools/[A-Za-z0-9_.-]+\.sh|python3|society)\b")

FLAGS_IN_BASH = re.compile(r"(?<![\w-])--[a-z][a-z0-9-]+")


def tracked_markdown() -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "*.md"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    return [ROOT / p for p in out if p]


def slug(text: str) -> str:
    """GitHub's heading anchor: lowercase, punctuation dropped, spaces to dashes."""
    text = re.sub(r"`([^`]*)`", r"\1", text)
    text = re.sub(r"\*\*|\*|__|_", "", text)
    text = text.strip().lower()
    text = re.sub(r"[^\w\s-]", "", text, flags=re.UNICODE)
    return re.sub(r"\s+", "-", text)


def headings(text: str) -> list[tuple[int, str]]:
    found, in_fence = [], False
    for line in text.splitlines():
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        m = HEADING.match(line)
        if m:
            found.append((len(m.group(1)), m.group(2)))
    return found


def check_structure(path: Path, text: str, problems: list[str], warnings: list[str]) -> None:
    heads = headings(text)
    if not heads:
        problems.append(f"{path}: no headings")
        return
    if [lvl for lvl, _ in heads if lvl == 1] and heads[0][0] != 1:
        problems.append(f"{path}: first heading is not H1")
    h1 = [t for lvl, t in heads if lvl == 1]
    if len(h1) > 1:
        problems.append(f"{path}: {len(h1)} H1 headings ({', '.join(h1)})")
    previous = 0
    for lvl, title in heads:
        if previous and lvl > previous + 1:
            problems.append(f"{path}: heading level jumps {previous}->{lvl} at `{title}`")
        previous = lvl
    fences = sum(1 for line in text.splitlines() if FENCE.match(line))
    if fences % 2:
        problems.append(f"{path}: unbalanced code fences ({fences})")
    in_fence = False
    for line in text.splitlines():
        m = FENCE.match(line)
        if not m:
            continue
        if not in_fence and not m.group(1):
            warnings.append(f"{path}: code fence without a language")
        in_fence = not in_fence


def check_targets(path: Path, text: str, problems: list[str]) -> None:
    anchors = {slug(t) for _, t in headings(text)}
    base = path.parent
    for raw in LINK.findall(text) + IMAGE.findall(text):
        target = raw.split("#", 1)[0]
        anchor = raw.split("#", 1)[1] if "#" in raw else ""
        if not target:
            if anchor and anchor not in anchors:
                problems.append(f"{path}: anchor `#{anchor}` has no heading")
            continue
        if re.match(r"^[a-z]+:", target) or target.startswith("//"):
            continue
        resolved = (base / target).resolve()
        if not resolved.exists():
            problems.append(f"{path}: link target `{target}` does not exist")
            continue
        if resolved.is_file() and resolved.suffix in {".gif", ".png", ".jpg", ".svg"}:
            size = resolved.stat().st_size
            if size > IMAGE_LIMIT:
                problems.append(
                    f"{path}: `{target}` is {size} bytes, over the {IMAGE_LIMIT} byte budget"
                )
        if resolved.is_file() and resolved.suffix == ".md" and anchor:
            other = {slug(t) for _, t in headings(resolved.read_text(encoding="utf-8"))}
            if anchor not in other:
                problems.append(f"{path}: anchor `{target}#{anchor}` has no heading")


def shell_blocks(text: str) -> list[str]:
    blocks, current, in_fence = [], [], False
    for line in text.splitlines():
        if FENCE.match(line):
            if in_fence and current:
                blocks.append("\n".join(current))
            current = []
            in_fence = not in_fence
            continue
        if in_fence:
            current.append(line)
    if current:
        blocks.append("\n".join(current))
    return blocks


def check_commands(path: Path, text: str, problems: list[str]) -> None:
    binary = ROOT / "target" / "release" / "society"
    help_text = ""
    if binary.exists():
        help_text = subprocess.run(
            [str(binary), "--help"], capture_output=True, text=True, check=False
        ).stdout
    accepted = set(FLAGS_IN_BASH.findall(help_text)) | {"--help"}
    for block in shell_blocks(text):
        for line in block.splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            for tool in SHELL_TOKEN.findall(stripped):
                if tool == "society":
                    continue
                candidate = ROOT / tool if "/" in tool else None
                if candidate and not candidate.exists():
                    problems.append(f"{path}: command names `{tool}`, which does not exist")
                elif candidate and candidate.suffix == ".sh" and not os.access(candidate, os.X_OK):
                    problems.append(f"{path}: `{tool}` is not executable")
            if not help_text:
                continue
            # `cargo run --release -- --view feed` sends only what follows `--`
            # to the binary; the rest is cargo's own flag space.
            words = stripped.split()
            # Environment assignments and a `cd` prefix are not the command.
            while words and ("=" in words[0] and not words[0].startswith("-")):
                words = words[1:]
            first = words[0] if words else ""
            binary_part = stripped
            if first in {"cargo", "python3", "bash", "sh"}:
                if not re.search(r"cargo\s+run", stripped):
                    continue
                parts = stripped.split(" -- ", 1)
                if len(parts) < 2:
                    continue
                binary_part = parts[1]
            elif Path(first).name != "society":
                continue
            for flag in FLAGS_IN_BASH.findall(binary_part):
                if flag not in accepted:
                    problems.append(
                        f"{path}: `{flag}` is not a flag of the `society` binary"
                    )


def main(argv: list[str]) -> int:
    paths = [Path(a) for a in argv[1:]] or tracked_markdown()
    problems: list[str] = []
    warnings: list[str] = []
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            problems.append(f"{path}: not UTF-8")
            continue
        check_structure(path, text, problems, warnings)
        check_targets(path, text, problems)
        check_commands(path, text, problems)
    for warning in warnings:
        print(f"warning: {warning}")
    for problem in problems:
        print(problem)
    print(f"check-docs: {len(paths)} file(s), {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
