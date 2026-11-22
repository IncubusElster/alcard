#!/usr/bin/env python3
"""Static build checks; never open TTY/input device or change host keymap."""
import gzip
import json
from pathlib import Path
import sys

def verify(layout_file, xkb_file, sources, generated):
    data = json.loads(Path(layout_file).read_text())
    assert data["schema"] == 1
    assert set(data["layouts"]) == {"us", "ru", "es", "de"}
    assert len(data["letters"]) == 26 and len(data["sym"]) == 32
    assert len({e["code"] for e in data["sym"]}) == 32
    # English original symbol characters must not be overwritten by language letters.
    assert not set(e["code"] for e in data["sym"]).intersection(
        e["code"] for e in data["letters"].values()
    )
    xkb = Path(xkb_file).read_text()
    us_source = (Path(sources) / "us.map").read_text()
    numeric_rows = [
        line for line in us_source.splitlines()
        if line.startswith("keycode ") and int(line.split()[1]) in range(2, 12)
    ]
    for language in data["layouts"]:
        assert f'xkb_symbols "{language}"' in xkb
        source = (Path(sources) / f"{language}.map").read_text()
        assert "keymaps 0-15" in source
        for number_row in numeric_rows:
            assert number_row in source, f"{language}: changed original number row"
        rows = {
            int(parts[1]): parts[3:]
            for line in source.splitlines()
            if line.startswith("keycode ")
            for parts in [line.split()]
        }
        for symbol in data["sym"]:
            code = symbol["code"]
            token = f"U+{ord(symbol['character']):04X}"
            assert rows[code] == [token] * 16, (
                f"{language}: original Sym keycode {code} changed"
            )
            assert f"key <{symbol['xkb']}>" in xkb
        target = Path(generated) / f"{language}.bmap.gz"
        with gzip.open(target, "rb") as stream:
            blob = stream.read()
        assert len(blob) > 20, language

if __name__ == "__main__":
    verify(*sys.argv[1:])
    print("Alcard Zero US/RU/ES/DE maps: static check OK")
