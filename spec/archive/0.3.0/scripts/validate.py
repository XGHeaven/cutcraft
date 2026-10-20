#!/usr/bin/env python3
"""Reference schema and semantic validator for Cutcraft Spec 0.3.0."""

from __future__ import annotations

import json
import math
import sys
from fractions import Fraction
from pathlib import Path
from typing import Any, Iterable

try:
    from jsonschema import Draft202012Validator, FormatChecker
    from referencing import Registry, Resource
except ImportError:
    print(
        "Missing dependency. Install with: python -m pip install 'jsonschema>=4.22'",
        file=sys.stderr,
    )
    raise SystemExit(2)


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_DIR = ROOT / "schemas"
EXAMPLE_DIR = ROOT / "examples"


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def pointer(parts: Iterable[str | int]) -> str:
    encoded = []
    for part in parts:
        encoded.append(str(part).replace("~", "~0").replace("/", "~1"))
    return "/" + "/".join(encoded)


def fraction(value: dict[str, int]) -> Fraction:
    return Fraction(value["num"], value["den"])


def range_bounds(value: dict[str, Any]) -> tuple[Fraction, Fraction]:
    start = fraction(value["start"])
    return start, start + fraction(value["duration"])


def walk_json(value: Any, path: tuple[str | int, ...] = ()):
    yield path, value
    if isinstance(value, dict):
        for key, child in value.items():
            yield from walk_json(child, path + (key,))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from walk_json(child, path + (index,))


def semantic_errors(project: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    assets = project["assets"]
    sequences = project["sequences"]
    for key in ("createdWith", "lastModifiedWith"):
        if project[key] not in project["editors"]:
            errors.append(f"/{key}: must be included in editors")
    for aid, asset in assets.items():
        if asset["kind"] == "lut" and asset["streams"]:
            errors.append(f"/assets/{aid}: LUT streams must be empty")

    if project["rootSequenceId"] not in sequences:
        errors.append("/rootSequenceId: referenced sequence does not exist")

    # All stored rational numbers must be in canonical reduced form.
    for path, value in walk_json(project):
        if (
            isinstance(value, dict)
            and set(value) == {"num", "den"}
            and isinstance(value["num"], int)
            and isinstance(value["den"], int)
            and value["den"] > 0
        ):
            if value["num"] == 0 and value["den"] != 1:
                errors.append(f"{pointer(path)}: zero time must be encoded as 0/1")
            elif value["num"] != 0 and math.gcd(abs(value["num"]), value["den"]) != 1:
                errors.append(f"{pointer(path)}: rational value is not reduced")

    stream_index: dict[tuple[str, str], dict[str, Any]] = {}
    for asset_id, asset in assets.items():
        seen_streams: set[str] = set()
        for index, stream in enumerate(asset["streams"]):
            stream_id = stream["id"]
            if stream_id in seen_streams:
                errors.append(
                    f"/assets/{asset_id}/streams/{index}/id: duplicate stream ID in asset"
                )
            seen_streams.add(stream_id)
            stream_index[(asset_id, stream_id)] = stream
            if stream["kind"] == "video" and "frameRate" not in stream["video"]:
                errors.append(
                    f"/assets/{asset_id}/streams/{index}/video: video stream requires frameRate"
                )

    # Entity IDs below asset/sequence maps are globally unique.
    global_ids: dict[str, str] = {}

    def claim(entity_id: str, path: str) -> None:
        previous = global_ids.get(entity_id)
        if previous is not None:
            errors.append(f"{path}: ID {entity_id!r} already used at {previous}")
        else:
            global_ids[entity_id] = path

    item_locations: dict[str, tuple[str, str, dict[str, Any]]] = {}
    sequence_edges: dict[str, set[str]] = {sequence_id: set() for sequence_id in sequences}

    for sequence_id, sequence in sequences.items():
        track_ids: list[str] = []
        for track_index, track in enumerate(sequence["tracks"]):
            track_path = f"/sequences/{sequence_id}/tracks/{track_index}"
            track_id = track["id"]
            claim(track_id, track_path + "/id")
            track_ids.append(track_id)

            canonical = sorted(
                track["items"],
                key=lambda item: (fraction(item["timelineRange"]["start"]), item["id"]),
            )
            if [item["id"] for item in canonical] != [
                item["id"] for item in track["items"]
            ]:
                errors.append(track_path + "/items: items are not in canonical order")

            enabled_ranges: list[tuple[Fraction, Fraction, str]] = []
            for item_index, item in enumerate(track["items"]):
                item_path = f"{track_path}/items/{item_index}"
                item_id = item["id"]
                claim(item_id, item_path + "/id")
                item_locations[item_id] = (sequence_id, track_id, item)
                start, end = range_bounds(item["timelineRange"])
                if item.get("enabled", True):
                    enabled_ranges.append((start, end, item_id))

                item_type = item["type"]
                allowed = {
                    "video": {"media", "sequence", "generator", "adjustment"},
                    "audio": {"media", "sequence", "generator", "adjustment"},
                    "subtitle": {"caption"},
                    "data": {"generator"},
                }[track["kind"]]
                if item_type not in allowed:
                    errors.append(
                        f"{item_path}/type: {item_type!r} is invalid on {track['kind']!r} track"
                    )

                if item_type == "media":
                    asset_id = item["assetId"]
                    stream_id = item["streamId"]
                    stream = stream_index.get((asset_id, stream_id))
                    if asset_id not in assets:
                        errors.append(f"{item_path}/assetId: asset does not exist")
                    elif stream is None:
                        errors.append(f"{item_path}/streamId: stream does not exist in asset")
                    else:
                        expected_track = {
                            "video": "video",
                            "image": "video",
                            "audio": "audio",
                            "subtitle": "subtitle",
                            "data": "data",
                        }[stream["kind"]]
                        if track["kind"] != expected_track:
                            errors.append(
                                f"{item_path}: {stream['kind']} stream cannot be placed on {track['kind']} track"
                            )
                        if stream["kind"] == "image":
                            if "sourceRange" in item:
                                errors.append(
                                    f"{item_path}/sourceRange: image item must omit sourceRange"
                                )
                        elif "sourceRange" not in item:
                            errors.append(
                                f"{item_path}/sourceRange: timed media item requires sourceRange"
                            )
                        elif "duration" in stream:
                            _, source_end = range_bounds(item["sourceRange"])
                            if source_end > fraction(stream["duration"]):
                                errors.append(
                                    f"{item_path}/sourceRange: range exceeds stream duration"
                                )
                        if (
                            item.get("playback", {}).get("mode") == "hold"
                            and stream["kind"] == "audio"
                        ):
                            errors.append(f"{item_path}/playback/mode: hold is invalid for audio")

                elif item_type == "sequence":
                    target = item["sequenceId"]
                    if target not in sequences:
                        errors.append(f"{item_path}/sequenceId: sequence does not exist")
                    else:
                        sequence_edges[sequence_id].add(target)

                elif item_type == "generator":
                    generator_type = item["generatorType"]
                    if generator_type in {"builtin.color", "builtin.text"} and track["kind"] != "video":
                        errors.append(
                            f"{item_path}/generatorType: {generator_type} requires a video track"
                        )
                    if generator_type == "builtin.silence" and track["kind"] != "audio":
                        errors.append(
                            f"{item_path}/generatorType: builtin.silence requires an audio track"
                        )

                duration = fraction(item["timelineRange"]["duration"])
                audio = item.get("audio", {})
                for field in ("fadeIn", "fadeOut"):
                    if field in audio and fraction(audio[field]) > duration:
                        errors.append(f"{item_path}/audio/{field}: fade exceeds item duration")

                for effect_index, effect in enumerate(item.get("effects", [])):
                    effect_path = f"{item_path}/effects/{effect_index}"
                    claim(effect["id"], effect_path + "/id")
                    kind = effect["effectType"]
                    if kind in {"builtin.color-grade", "builtin.curves", "builtin.lut"}:
                        if track["kind"] != "video":
                            errors.append(effect_path + ": color effects require video track")
                        if effect.get("automation"):
                            errors.append(effect_path + ": color v1 does not support automation")
                    if kind == "builtin.lut":
                        aid = effect["parameters"]["assetId"]
                        if assets.get(aid, {}).get("kind") != "lut":
                            errors.append(effect_path + ": missing LUT asset")
                    if kind == "builtin.curves":
                        for points in effect["parameters"].values():
                            xs = [p["x"] for p in points]
                            if xs[0] != 0 or xs[-1] != 1 or any(a >= b for a,b in zip(xs,xs[1:])):
                                errors.append(effect_path + ": curve x must increase from 0 to 1")
                    for automation_index, automation in enumerate(effect.get("automation", [])):
                        keyframes = automation["keyframes"]
                        offsets = [fraction(keyframe["offset"]) for keyframe in keyframes]
                        if offsets != sorted(offsets):
                            errors.append(
                                f"{effect_path}/automation/{automation_index}/keyframes: offsets are not sorted"
                            )
                        if offsets and offsets[-1] > duration:
                            errors.append(
                                f"{effect_path}/automation/{automation_index}/keyframes: offset exceeds item duration"
                            )

                if item_type == "caption":
                    word_ranges = []
                    for word_index, word in enumerate(item.get("words", [])):
                        word_start, word_end = range_bounds(word["range"])
                        word_ranges.append((word_start, word_end))
                        if word_end > duration:
                            errors.append(
                                f"{item_path}/words/{word_index}/range: word exceeds caption duration"
                            )
                    if word_ranges != sorted(word_ranges):
                        errors.append(f"{item_path}/words: words are not in time order")

            if track["overlapPolicy"] != "allow":
                enabled_ranges.sort()
                for previous, current in zip(enabled_ranges, enabled_ranges[1:]):
                    if current[0] < previous[1]:
                        errors.append(
                            f"{track_path}/items: {previous[2]!r} overlaps {current[2]!r}"
                        )

        if len(track_ids) != len(set(track_ids)):
            errors.append(f"/sequences/{sequence_id}/tracks: duplicate track IDs")

        for index, transition in enumerate(sequence["transitions"]):
            transition_path = f"/sequences/{sequence_id}/transitions/{index}"
            claim(transition["id"], transition_path + "/id")
            from_location = item_locations.get(transition["fromItemId"])
            to_location = item_locations.get(transition["toItemId"])
            if from_location is None or from_location[0] != sequence_id:
                errors.append(transition_path + "/fromItemId: item not found in sequence")
            if to_location is None or to_location[0] != sequence_id:
                errors.append(transition_path + "/toItemId: item not found in sequence")
            if from_location is not None and to_location is not None:
                if from_location[1] != to_location[1]:
                    errors.append(transition_path + ": transition items must share a track")
                _, from_end = range_bounds(from_location[2]["timelineRange"])
                to_start, _ = range_bounds(to_location[2]["timelineRange"])
                if from_end != to_start:
                    errors.append(transition_path + ": transition items are not adjacent")

        for index, link in enumerate(sequence["links"]):
            link_path = f"/sequences/{sequence_id}/links/{index}"
            claim(link["id"], link_path + "/id")
            for item_id in link["itemIds"]:
                location = item_locations.get(item_id)
                if location is None or location[0] != sequence_id:
                    errors.append(f"{link_path}/itemIds: item {item_id!r} not found in sequence")
                elif location[2].get("linkGroupId") != link["id"]:
                    errors.append(
                        f"{link_path}/itemIds: item {item_id!r} does not point back to link group"
                    )

        for index, marker in enumerate(sequence["markers"]):
            claim(marker["id"], f"/sequences/{sequence_id}/markers/{index}/id")

    # Check dangling item-to-link references after all sequence links are indexed.
    valid_links = {
        link["id"]
        for sequence in sequences.values()
        for link in sequence["links"]
    }
    for item_id, (_, _, item) in item_locations.items():
        if "linkGroupId" in item and item["linkGroupId"] not in valid_links:
            errors.append(f"item {item_id!r}: linkGroupId does not exist")

    # Detect nested sequence cycles.
    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(sequence_id: str, stack: list[str]) -> None:
        if sequence_id in visiting:
            cycle = " -> ".join(stack + [sequence_id])
            errors.append(f"/sequences: nested sequence cycle detected: {cycle}")
            return
        if sequence_id in visited:
            return
        visiting.add(sequence_id)
        for target in sequence_edges.get(sequence_id, set()):
            visit(target, stack + [sequence_id])
        visiting.remove(sequence_id)
        visited.add(sequence_id)

    for sequence_id in sequences:
        visit(sequence_id, [])

    return errors


def event_semantic_errors(event: dict[str, Any]) -> list[str]:
    before = event["revisionBefore"]
    after = event["revisionAfter"]
    if event["status"] == "applied" and after != before + 1:
        return ["applied event must increment revision by exactly one"]
    if event["status"] == "rejected" and after != before:
        return ["rejected event must not change revision"]
    return []


def main() -> int:
    schemas = {
        path.name: load_json(path)
        for path in sorted(SCHEMA_DIR.glob("*.schema.json"))
    }
    for schema in schemas.values():
        Draft202012Validator.check_schema(schema)
    registry = Registry().with_resources(
        [
            (schema["$id"], Resource.from_contents(schema))
            for schema in schemas.values()
        ]
    )
    validators = {
        name: Draft202012Validator(
            schema,
            registry=registry,
            format_checker=FormatChecker(),
        )
        for name, schema in schemas.items()
    }

    failed = False

    for file_name in ("minimal.project.json", "full.project.json"):
        path = EXAMPLE_DIR / file_name
        value = load_json(path)
        schema_errors = sorted(
            validators["project.schema.json"].iter_errors(value),
            key=lambda error: list(error.absolute_path),
        )
        semantic = semantic_errors(value) if not schema_errors else []
        if schema_errors or semantic:
            failed = True
            print(f"FAIL {file_name}")
            for error in schema_errors:
                print(f"  schema {pointer(error.absolute_path)}: {error.message}")
            for error in semantic:
                print(f"  semantic {error}")
        else:
            print(f"PASS {file_name}")

    operation_path = EXAMPLE_DIR / "batch.operation.json"
    operation = load_json(operation_path)
    operation_errors = list(validators["operation.schema.json"].iter_errors(operation))
    if operation_errors:
        failed = True
        print("FAIL batch.operation.json")
        for error in operation_errors:
            print(f"  schema {pointer(error.absolute_path)}: {error.message}")
    else:
        print("PASS batch.operation.json")

    history_path = EXAMPLE_DIR / "history.jsonl"
    with history_path.open("r", encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, start=1):
            if not line.strip():
                continue
            event = json.loads(line)
            event_errors = list(validators["event.schema.json"].iter_errors(event))
            semantic = event_semantic_errors(event) if not event_errors else []
            if event_errors or semantic:
                failed = True
                print(f"FAIL history.jsonl:{line_number}")
                for error in event_errors:
                    print(f"  schema {pointer(error.absolute_path)}: {error.message}")
                for error in semantic:
                    print(f"  semantic {error}")
            else:
                print(f"PASS history.jsonl:{line_number}")

    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
