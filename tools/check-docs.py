#!/usr/bin/env python3
"""Check local Markdown links and the structural parity of EN/RU documentation.

Standard library only; no network, Cargo, package build or system mutation.
This catches missing targets and translation drift, not semantic correctness.
"""

from collections import Counter
import os
from pathlib import Path
import re
import sys
import unicodedata
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]
TOPICS = {
    "overview", "building", "installation", "settings", "desktop", "security",
    "power-audio", "hardware", "boot-updates", "packaging", "fonts-keyboard",
    "testing", "roadmap", "status", "licensing",
}
EXCLUDED = {".git", ".alcard-local", "target", "__pycache__", "node_modules",
            ".venv", "upstream", "build", "dist"}
FENCE = re.compile(r"^```([^\n]*)\n(.*?)^```[ \t]*$", re.M | re.S)
LINK = re.compile(r"!?\[[^\]\n]*\]\((<[^>]+>|[^)\n]+)\)")
RUNNABLE = {"sh", "bash", "console", "python", "json", "toml", "ini", "rust"}


def markdown_files():
    for base, dirs, files in os.walk(ROOT):
        dirs[:] = sorted(name for name in dirs if name not in EXCLUDED)
        for name in sorted(files):
            if name.endswith(".md"):
                yield Path(base) / name


def prose(text):
    return FENCE.sub("", text)


def anchors(text):
    body = prose(text)
    result = set(re.findall(r'<a\s+[^>]*?(?:id|name)=["\x27]([^"\x27]+)', body))
    seen = Counter()
    for line in body.splitlines():
        heading = re.match(r"^#{1,6}\s+(.+?)(?:\s+#+)?$", line)
        if not heading:
            continue
        value = re.sub(r"<[^>]*>", "", heading.group(1)).strip().lower()
        value = "".join(c for c in value if c in "-_ " or
                        unicodedata.category(c)[0] in "LNM")
        value = value.replace(" ", "-")
        suffix = seen[value]
        seen[value] += 1
        result.add(value if suffix == 0 else f"{value}-{suffix}")
    return result


def commands(text):
    result = []
    for match in FENCE.finditer(text):
        language = match.group(1).strip().lower()
        if language not in RUNNABLE:
            continue
        lines = [line.rstrip() for line in match.group(2).splitlines()
                 if line.strip() and not line.lstrip().startswith(("#", "//"))]
        result.append((language, lines))
    return result


def main():
    errors = []
    sources = {path: path.read_text(encoding="utf-8") for path in markdown_files()}
    link_count = 0
    for path, text in sources.items():
        for match in LINK.finditer(prose(text)):
            raw = match.group(1).strip()
            raw = raw[1:-1] if raw.startswith("<") else raw.split()[0]
            parts = urlsplit(raw)
            if parts.scheme or parts.netloc:
                continue
            link_count += 1
            target = (path.parent / unquote(parts.path)).resolve() if parts.path else path
            label = str(path.relative_to(ROOT))
            if not target.is_relative_to(ROOT) or not target.exists():
                errors.append(f"{label}: missing local link target: {raw}")
                continue
            if parts.fragment and target.suffix == ".md":
                target_text = sources.get(target)
                if target_text is None:
                    target_text = target.read_text(encoding="utf-8")
                if unquote(parts.fragment) not in anchors(target_text):
                    errors.append(f"{label}: missing local heading anchor: {raw}")

    pairs = []
    for language in ("en", "ru"):
        found = {p.stem for p in (ROOT / "docs" / language).glob("*.md")}
        if found != TOPICS:
            errors.append(f"docs/{language}: topic set differs: {sorted(found ^ TOPICS)}")
    for name in sorted(TOPICS):
        en = ROOT / "docs/en" / f"{name}.md"
        ru = ROOT / "docs/ru" / f"{name}.md"
        if en not in sources or ru not in sources:
            errors.append(f"Missing EN/RU pair: {name}")
            continue
        if f"../ru/{name}.md" not in sources[en] or f"../en/{name}.md" not in sources[ru]:
            errors.append(f"Missing reciprocal language link: {name}")
        pairs.append((en, ru))
    for name in ("README", "CONTRIBUTING", "SECURITY"):
        en, ru = ROOT / f"{name}.md", ROOT / f"{name}.ru.md"
        if en not in sources or ru not in sources:
            errors.append(f"Missing root EN/RU pair: {name}")
        else:
            pairs.append((en, ru))
    for en, ru in pairs:
        if commands(sources[en]) != commands(sources[ru]):
            errors.append(f"EN/RU runnable code blocks differ: {en.relative_to(ROOT)}")
        levels = lambda text: Counter(len(m.group(1)) for m in
                                      re.finditer(r"^(#{1,6})\s", prose(text), re.M))
        if levels(sources[en]) != levels(sources[ru]):
            errors.append(f"EN/RU heading structure differs: {en.relative_to(ROOT)}")
        pins = lambda text: Counter(re.findall(r"`([0-9a-f]{7,40})`", text))
        if pins(sources[en]) != pins(sources[ru]):
            errors.append(f"EN/RU source/commit identifiers differ: {en.relative_to(ROOT)}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"PASS: {len(sources)} Markdown files, {link_count} local links, "
          f"{len(pairs)} EN/RU pairs; no missing local targets/anchors or structural drift")
    print("External URLs and translation semantics require review; no network was used.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
