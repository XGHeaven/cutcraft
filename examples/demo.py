#!/usr/bin/env python3
"""Build a playable demo through the public CLI. Python is only for this example."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
BINARY = shutil.which("cutcraft") or str(ROOT / "target/debug/cutcraft")
DESTINATION = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / "local-examples/demo.cut").resolve()

def run(*args, value=None):
    result = subprocess.run([BINARY, "--project", str(DESTINATION), *map(str, args)],
                            input=json.dumps(value) if value is not None else None,
                            text=True, check=True, capture_output=True)
    return json.loads(result.stdout)

def new_id(prefix):
    return run("id", prefix)["id"]

def time(n):
    return {"num": n, "den": 1}

def span(start, duration):
    return {"start": time(start), "duration": time(duration)}

def color(r, g, b):
    return {"r": r, "g": g, "b": b, "a": 1}

def item(kind, start, duration, **fields):
    return {"id": new_id("item"), "type": kind, "timelineRange": span(start, duration), **fields}

def track(name, kind, items):
    return {"id": new_id("track"), "kind": kind, "name": name, "overlapPolicy": "forbid", "items": items}

project = run("init", DESTINATION, "--name", "Cutcraft · Agent-first editing")["project"]
sid = project["rootSequenceId"]
sequence = project["sequences"][sid]
sequence["format"].update(width=960, height=540)
sequence["tracks"] = [
    track("Background", "video", [
        item("generator", 0, 3, generatorType="builtin.color", version="1.0.0", parameters={"color": color(.06, .12, .19)}),
        item("generator", 3, 3, generatorType="builtin.color", version="1.0.0", parameters={"color": color(.09, .22, .22)}),
    ]),
    track("Title", "video", [item("generator", 0, 6, generatorType="builtin.text", version="1.0.0", parameters={"text": "CUTCRAFT", "fontSizePixels": 76, "position": "center", "color": color(.9, .97, 1)})]),
    track("Captions", "subtitle", [
        item("caption", 0, 3, text="Exact time. Atomic edits.", style={"fontSizePixels": 28}),
        item("caption", 3, 3, text="Built for agents. Previewed by you.", style={"fontSizePixels": 28}),
    ]),
]
sequence["markers"] = [{"id": new_id("marker"), "name": "Scene change", "start": time(3), "duration": time(0)}]
run("command", "-", value={"type": "sequence.put", "sequenceId": sid, "sequence": sequence})
preview = run("preview")
print(json.dumps(preview, indent=2))
