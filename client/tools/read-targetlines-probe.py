#!/usr/bin/env python3
"""Summarize a bounded Targetlines camera/draw diagnostic capture."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct


FILE_HEADER = struct.Struct("<8s65sQ16sq")
RECORD_HEADER = struct.Struct("<IIQqIII")
CAMERA = struct.Struct("<Q64f")
DRAW = struct.Struct("<Q5I2f10I6f")
DRAW_V2 = struct.Struct("<Q5I2f10I6fI")
MAXIMUM_BYTES = 128 * 1024 * 1024
CLIENT_SHA256 = "9341f2b4567440b310a4d494f5cc5599ca334ba51c8042247317ff466492f2e9"


def summarize(path):
    path = Path(path)
    if path.stat().st_size > MAXIMUM_BYTES:
        raise ValueError("capture exceeds the diagnostic size limit")
    counts = Counter()
    frames = set()
    generations = set()
    shaders = Counter()
    views = Counter()
    surfaces = Counter()
    zones = set()
    camera_sequences = set()
    positioned_draws = 0
    back_buffer_draws = 0
    incomplete = False
    with path.open("rb") as capture:
        header = capture.read(FILE_HEADER.size)
        if len(header) != FILE_HEADER.size:
            raise ValueError("missing capture header")
        magic, digest, length, version, frequency = FILE_HEADER.unpack(header)
        if (magic not in (b"BTLP0001", b"BTLP0002")
                or digest.rstrip(b"\0").decode("ascii") != CLIENT_SHA256
                or length != 15996808 or version.rstrip(b"\0") != b"2012.09.19.0001"
                or frequency <= 0):
            raise ValueError("unsupported capture format or client identity")
        draw_layout = DRAW if magic == b"BTLP0001" else DRAW_V2
        format_version = 1 if magic == b"BTLP0001" else 2
        while raw_header := capture.read(RECORD_HEADER.size):
            if len(raw_header) != RECORD_HEADER.size:
                incomplete = True
                break
            kind, size, sequence, counter, thread, frame, generation = RECORD_HEADER.unpack(raw_header)
            if size > draw_layout.size + 256 * 16 + 16 * 1024:
                raise ValueError("oversized record")
            payload = capture.read(size)
            if len(payload) != size:
                incomplete = True
                break
            frames.add(frame)
            generations.add(generation)
            if kind == 1:
                if size != CAMERA.size:
                    raise ValueError("invalid camera record size")
                camera_sequence, *records = CAMERA.unpack(payload)
                if camera_sequence != sequence:
                    raise ValueError("camera sequence does not match its record")
                camera_sequences.add(camera_sequence)
            elif kind == 2:
                if size < draw_layout.size:
                    raise ValueError("missing draw metadata")
                values = draw_layout.unpack_from(payload)
                camera_sequence, surface, x, y, width, height, min_z, max_z = values[:8]
                render_width, render_height, back_width, back_height = values[8:12]
                constant_count, shader_bytes, source, target, zone, has_positions = values[12:18]
                if (constant_count < 4 or constant_count > 256 or shader_bytes == 0
                        or shader_bytes > 16 * 1024
                        or size != draw_layout.size + constant_count * 16 + shader_bytes
                        or camera_sequence not in camera_sequences):
                    raise ValueError("invalid draw data or missing camera record")
                is_back_buffer = None
                if format_version == 2:
                    if values[24] not in (0, 1):
                        raise ValueError("invalid back-buffer comparison")
                    is_back_buffer = bool(values[24])
                    back_buffer_draws += int(is_back_buffer)
                shader = payload[draw_layout.size + constant_count * 16:]
                shaders[hashlib.sha256(shader).hexdigest()] += 1
                views[(x, y, width, height, min_z, max_z)] += 1
                surfaces[(surface, render_width, render_height, back_width, back_height,
                          is_back_buffer)] += 1
                if has_positions:
                    if not source or not target or not zone:
                        raise ValueError("invalid positioned draw identity")
                    positioned_draws += 1
                    zones.add(zone)
            elif kind in (3, 4):
                if size:
                    raise ValueError("unexpected frame/reset payload")
                if kind == 4:
                    camera_sequences.clear()
            else:
                raise ValueError("unknown capture record")
            counts[kind] += 1
    return {
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "bytes": path.stat().st_size,
        "format_version": format_version,
        "camera_records": counts[1],
        "draw_records": counts[2],
        "present_records": counts[3],
        "reset_records": counts[4],
        "positioned_draws": positioned_draws,
        "back_buffer_draws": back_buffer_draws if format_version == 2 else None,
        "frames": len(frames),
        "generations": sorted(generations),
        "zones": sorted(zones),
        "shader_sha256": dict(shaders),
        "viewports": [{"values": list(key), "samples": count} for key, count in views.items()],
        "surfaces": [{"values": list(key[:5]), "is_back_buffer": key[5], "samples": count}
                     for key, count in surfaces.items()],
        "incomplete_tail": incomplete,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    args = parser.parse_args()
    try:
        summary = summarize(args.capture)
    except (OSError, ValueError, UnicodeError) as error:
        parser.exit(1, f"{error}\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
