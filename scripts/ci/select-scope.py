#!/usr/bin/env python3
"""Select static, source, acceptance or an explicit Hello continuation.

No workflow-level path filter can leave a required check permanently pending.
Unknown paths and unavailable ancestry conservatively select the source gate.
"""

import json
import os
from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[2]
SHA = re.compile(r"[0-9a-f]{40}\Z")
LEGAL_FILES = {"LICENSE", "NOTICE"}


def documentation_only(paths):
    return bool(paths) and all(
        path.endswith(".md") or path in LEGAL_FILES or path.startswith("licenses/")
        for path in paths
    )


def choose_scope(event_name, event, mode, git):
    if event_name == "workflow_dispatch":
        if mode not in {"source", "acceptance", "hello"}:
            raise ValueError("Manual CI mode must be source, acceptance or hello")
        baseline = event.get("inputs", {}).get("baseline_run", "")
        if mode == "hello":
            if not isinstance(baseline, str) or not re.fullmatch(r"[1-9][0-9]{0,19}", baseline):
                raise ValueError("Hello continuation requires a numeric baseline_run")
        elif baseline:
            raise ValueError("baseline_run is only valid for Hello continuation")
        return mode, [], "Explicit manual selection"
    if event_name == "pull_request":
        base = event.get("pull_request", {}).get("base", {}).get("sha", "")
        # GitHub checks out the merge result. Compare it with the current base
        # so that changes already present on the target branch are excluded.
    elif event_name == "push":
        base = event.get("before", "")
    else:
        return "source", [], "Unknown event requires source checks"
    if not SHA.fullmatch(base) or base == "0" * 40:
        return "source", [], "New history or missing base requires source checks"
    try:
        git("cat-file", "-e", base + "^{commit}")
        paths = [p for p in git("diff", "--name-only", "-z", base, "HEAD", "--").split("\0") if p]
    except subprocess.CalledProcessError:
        return "source", [], "Unavailable comparison base requires source checks"
    scope = "static" if documentation_only(paths) else "source"
    return scope, paths, "Documentation/license-only change" if scope == "static" else "Code, build or unknown input changed"


def main():
    def git(*arguments):
        return subprocess.check_output(["git", *arguments], cwd=ROOT, text=True,
                                       stderr=subprocess.DEVNULL)

    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    scope, paths, reason = choose_scope(os.environ["GITHUB_EVENT_NAME"], event,
                                       os.environ.get("ALCARD_CI_MODE", "source"), git)
    output = ROOT / ".alcard-local/ci"
    output.mkdir(parents=True, exist_ok=True)
    (output / "selection.json").write_text(json.dumps({
        "scope": scope, "reason": reason, "changed_paths": paths,
        "source_sha": git("rev-parse", "HEAD").strip(),
        "source_tree": git("rev-parse", "HEAD^{tree}").strip(),
        "event": os.environ["GITHUB_EVENT_NAME"],
    }, indent=2) + "\n")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
        stream.write(f"scope={scope}\n")
    print(f"Selected {scope}: {reason}")


if __name__ == "__main__":
    main()
