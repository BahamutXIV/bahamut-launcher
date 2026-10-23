#!/usr/bin/env python3
"""Build the exact-stock seasonal TimeLine metadata catalog.

This generator reads only the six pinned MapLayout DATs named below.  It
emits offsets, names, hashes and SCB fingerprints, never serialized payload
bytes.  The input root is always explicit so a different client cannot be
selected by discovery or a sibling-repository default.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import stat
import struct
import sys
from typing import Iterable


MAP_MAGIC = b"MapLayoutResourceData" + b"\0" * 3
LYB_MAGIC = b"SEDBlyb\0"
SCB_MAGIC = b"SEDBSCB\0"
TIME_LINE = (0x28, ((0x04, 0), (0x08, 2), (0x14, 0), (0x1C, 0), (0x20, 2)))
PROPERTY = (0x18, ((0x04, 0), (0x08, 2), (0x10, 0)))
PROPERTY_SIZE = PROPERTY[0]
VECTOR_ITEM = (0x08, ())
UNIT_TREE = (0x50, ((0x04, 0), (0x08, 2), (0x1C, 0), (0x20, 0), (0x24, 0), (0x2C, 2), (0x30, 0)))
CHILD = (0x30, ((0x0C, 0), (0x10, 0), (0x14, 2), (0x18, 0), (0x20, 0)))
CLASS_NODE = (0x14, ((0x04, 0), (0x08, 2), (0x10, 0)))
WEATHER_IDS = tuple(range(8001, 8018)) + tuple(range(8027, 8033))
FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3


class CatalogError(ValueError):
    """A pinned source or typed catalog relation was invalid."""


@dataclass(frozen=True)
class Source:
    key: str
    resource_id: int
    root: int
    relative_path: str
    size: int
    sha256: str
    pair_suffixes: tuple[str, ...]
    family: str


SOURCES = (
    Source(
        "public_limsa",
        0x29D90001,
        101,
        "data/29/D9/00/01.DAT",
        1197104,
        "bc40d7ff57ab8998978532cce4564e2488ed81d6d10b8f7ede3e1586b0b26523",
        tuple(f"hw_obj{i}_h" for i in range(1, 9)),
        "public_halloween",
    ),
    Source(
        "public_gridania",
        0x29B00001,
        103,
        "data/29/B0/00/01.DAT",
        1300928,
        "abc13bd6f096b0fe9703b2092ca4a7f819ff2b88e4d08ae222a9db447f618312",
        ("hlw1a", "hlw1b", "hlw1c", "hlw1d", "hlw1e", "hlw1f", "hlw1g", "hlw1h", "hlw1i", "hlw2a", "hlw2b"),
        "public_halloween",
    ),
    Source(
        "public_uldah",
        0x615A0001,
        104,
        "data/61/5A/00/01.DAT",
        1036816,
        "69c6ff8488ed9c407a28705cf6cb844d55845f7eed3ad58b31845b11a8331c4c",
        tuple(f"hw{i}" for i in range(10)),
        "public_halloween",
    ),
    Source(
        "interior_limsa",
        0x29D90002,
        101,
        "data/29/D9/00/02.DAT",
        728976,
        "c6d86bfea2692e612464e7e08f6604df4f26fc1ea99da16727e824c920a693d8",
        ("hw_aq1_h", "hw_ast_h", "hw_f1_h", "hw_f2_h", "hw_f3_h", "hw_hjt_h", "hw_kos_h", "hw_main_h", "hw_sp1_h", "hw_sp2_h", "hw_tik_h", "hw_ws1_h", "hw_ws2_h", "hw_obj8_h"),
        "interior_halloween",
    ),
    Source(
        "interior_gridania",
        0x29B00002,
        103,
        "data/29/B0/00/02.DAT",
        460368,
        "f2088d1feba5fe71fbc4c462229f1bd1970371d6f023622a7a83d6aa8d450876",
        ("hwa1", "hwf1", "hwf2", "hww1", "hww2", "hww3", "hws1", "hws2", "hws3", "hws4"),
        "interior_halloween",
    ),
    Source(
        "interior_uldah",
        0x615A0003,
        104,
        "data/61/5A/00/03.DAT",
        529616,
        "d969e982a55d5c986e7bea24ae5af1bf23bfe7ed18854f7dd7c4f21a11d2353f",
        ("hwa1", "hwf3", "hwf2", "hwf1", "hwg1", "hwe1", "hwe2"),
        "interior_halloween",
    ),
)

WINTER_PAIRS = {
    "interior_gridania": ("crs1",),
    "interior_limsa": ("itm0_pstr1_h", "itm0_pstr3_h"),
    "interior_uldah": ("xmas1", "xmas2"),
}


@dataclass(frozen=True)
class NodeType:
    start: int
    count: int
    size: int
    fields: tuple[tuple[int, int], ...]

    @property
    def shape(self) -> tuple[int, tuple[tuple[int, int], ...]]:
        return self.size, self.fields

    @property
    def end(self) -> int:
        return self.start + self.count * self.size

    def nodes(self) -> range:
        return range(self.start, self.end, self.size)


class Reader:
    def __init__(self, data: bytes, label: str):
        self.data = data
        self.label = label

    def span(self, offset: int, size: int) -> bytes:
        if offset < 0 or size < 0 or offset > len(self.data) or size > len(self.data) - offset:
            raise CatalogError(f"{self.label}: out-of-bounds span offset=0x{offset:X} size=0x{size:X}")
        return self.data[offset : offset + size]

    def u16(self, offset: int) -> int:
        return struct.unpack_from("<H", self.span(offset, 2))[0]

    def u32(self, offset: int) -> int:
        return struct.unpack_from("<I", self.span(offset, 4))[0]

    def string(self, offset: int) -> str:
        if offset == 0:
            return ""
        self.span(offset, 1)
        end = self.data.find(b"\0", offset)
        if end < 0:
            raise CatalogError(f"{self.label}: unterminated string at 0x{offset:X}")
        value = self.data[offset:end]
        if any(byte < 0x20 or byte > 0x7E for byte in value):
            raise CatalogError(f"{self.label}: non-ASCII string at 0x{offset:X}")
        return value.decode("ascii")


@dataclass
class Layout:
    source: Source
    data: bytes
    payload: Reader
    payload_file_offset: int
    node_types: dict[int, NodeType]
    types: tuple[NodeType, ...]
    class_paths: dict[int, str]

    def kind(self, node: int) -> NodeType:
        try:
            return self.node_types[node]
        except KeyError as error:
            raise CatalogError(f"{self.source.key}: reference is not a declared node 0x{node:X}") from error

    def string_ref(self, node: int, offset: int) -> tuple[int, str]:
        value = self.payload.u32(node + offset)
        return value, self.payload.string(value)

    def run(self, offset: int, count: int, expected: tuple[int, tuple[tuple[int, int], ...]] | None = None) -> tuple[int, ...]:
        if count == 0:
            if offset != 0:
                raise CatalogError(f"{self.source.key}: null run has nonzero offset 0x{offset:X}")
            return ()
        kind = self.kind(offset)
        if expected is not None and kind.shape != expected:
            raise CatalogError(f"{self.source.key}: run at 0x{offset:X} has unexpected shape")
        end = offset + count * kind.size
        if end > kind.end:
            raise CatalogError(f"{self.source.key}: run at 0x{offset:X} escapes declared extent")
        self.payload.span(offset, count * kind.size)
        return tuple(range(offset, end, kind.size))


def parse_layout(source: Source, data: bytes) -> Layout:
    raw = Reader(data, source.key)
    if raw.span(0, 24) != MAP_MAGIC:
        raise CatalogError(f"{source.key}: MapLayout signature mismatch")
    scene = raw.u32(0x20)
    row_count = raw.u32(0x24)
    if 0x40 + row_count * 0x20 != scene:
        raise CatalogError(f"{source.key}: resource table does not end at scene offset")
    raw.span(0x40, row_count * 0x20)
    if raw.span(scene, 8) != LYB_MAGIC:
        raise CatalogError(f"{source.key}: missing lyb SEDB container")
    header_size = raw.u16(scene + 0x0E)
    total_size = raw.u32(scene + 0x10)
    if header_size < 0x14 or total_size < header_size:
        raise CatalogError(f"{source.key}: invalid lyb header")
    raw.span(scene, total_size)
    payload_file_offset = scene + header_size
    payload = Reader(raw.span(payload_file_offset, total_size - header_size), source.key + ":lyb")
    if payload.span(0, 4) != b"lyb\0" or payload.u32(4) != total_size:
        raise CatalogError(f"{source.key}: invalid lyb payload header")
    type_table = payload.u32(0x10)
    object_list = payload.u32(0x14)
    if object_list < 0x18 or type_table < object_list or (type_table - object_list) % 4:
        raise CatalogError(f"{source.key}: invalid object-list range")
    object_values = (type_table - object_list) // 4
    payload.span(object_list, object_values * 4)
    if object_values == 0:
        raise CatalogError(f"{source.key}: empty object list")
    type_table_end = payload.u32(type_table + 4)
    if type_table_end <= type_table or (type_table_end - type_table) % 16:
        raise CatalogError(f"{source.key}: invalid node-type table range")
    payload.span(type_table, type_table_end - type_table)
    types: list[NodeType] = []
    node_types: dict[int, NodeType] = {}
    for row in range(type_table, type_table_end, 16):
        extent_ref, fields_ref, _one, field_count = struct.unpack_from("<IIII", payload.span(row, 16))
        extent_start, node_count, node_size = struct.unpack_from("<III", payload.span(extent_ref, 12))
        if node_size == 0:
            raise CatalogError(f"{source.key}: zero node size")
        fields = tuple(struct.unpack_from("<HH", payload.span(fields_ref + i * 4, 4)) for i in range(field_count))
        fields = tuple(sorted(fields))
        node_type = NodeType(extent_start, node_count, node_size, fields)
        payload.span(extent_start, node_count * node_size)
        types.append(node_type)
        for node in node_type.nodes():
            if node in node_types:
                raise CatalogError(f"{source.key}: overlapping node extents at 0x{node:X}")
            node_types[node] = node_type
    class_paths: dict[int, str] = {}
    for node_type in types:
        if node_type.shape != CLASS_NODE:
            continue
        for node in node_type.nodes():
            class_paths[node] = payload.string(payload.u32(node + 8))
    return Layout(source, data, payload, payload_file_offset, node_types, tuple(types), class_paths)


def fnv1a64(value: bytes) -> int:
    result = FNV_OFFSET
    for byte in value:
        result ^= byte
        result = (result * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return result


def expected_vector_fixed(role: str) -> tuple[tuple[int, int], ...]:
    if role == "HalloweenShow":
        selected_ids = {8027}
    elif role == "HalloweenHide":
        selected_ids = set(WEATHER_IDS) - {8027}
    elif role == "WinterShow":
        selected_ids = set()
    elif role == "WinterHide":
        selected_ids = set(WEATHER_IDS)
    else:
        raise CatalogError(f"unknown role {role}")
    return tuple(
        (weather_id | 0xFFFF0000, 0xFFFFFFFF) if weather_id in selected_ids else (weather_id, 0)
        for weather_id in WEATHER_IDS
    )


def role_for(family: str, side: str) -> str:
    if side not in ("show", "hide"):
        raise CatalogError(f"unexpected seasonal side {side}")
    if family in ("public_halloween", "interior_halloween"):
        return "HalloweenShow" if side == "show" else "HalloweenHide"
    if family == "winter":
        return "WinterShow" if side == "show" else "WinterHide"
    raise CatalogError(f"unexpected seasonal family {family}")


def selected_names(source: Source) -> tuple[str, ...]:
    suffixes = source.pair_suffixes + tuple(WINTER_PAIRS.get(source.key, ()))
    names: list[str] = []
    for suffix in suffixes:
        prefix = f"time_bg_{suffix}"
        names.extend((prefix + "_show", prefix + "_hide"))
    return tuple(names)


@dataclass(frozen=True)
class Record:
    source: Source
    role: str
    timeline_offset: int
    timeline_size: int
    owner_offset: int
    owner_size: int
    owner_child_offset: int
    owner_child_size: int
    owner_child_name_offset: int
    property_offset: int
    property_size: int
    vector_offset: int
    vector_size: int
    vector_count: int
    name_offset: int
    owner_name_offset: int
    property_name_offset: int
    scb_offset: int
    scb_size: int
    timeline_name: str
    owner_name: str
    owner_child_name: str
    property_name: str
    scb_sha256: str
    scb_fingerprint: int


def source_path(root: Path, relative: str) -> Path:
    return root.joinpath(*relative.split("/"))


def reparse_attributes(path: Path) -> int:
    try:
        info = path.lstat()
    except OSError as error:
        raise CatalogError(f"cannot stat {path}: {error}") from error
    return int(getattr(info, "st_file_attributes", 0))


def assert_no_reparse(path: Path, root: Path) -> None:
    root = root.absolute()
    path = path.absolute()
    try:
        relative = path.relative_to(root)
    except ValueError as error:
        raise CatalogError(f"source path escapes client root: {path}") from error
    paths = [root]
    current = root
    for part in relative.parts:
        current /= part
        paths.append(current)
    for current in paths:
        if current.is_symlink() or reparse_attributes(current) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400):
            raise CatalogError(f"reparse point is not an accepted source: {current}")


def load_sources(client_root: Path) -> dict[str, Layout]:
    root = client_root.absolute()
    assert_no_reparse(root, root.parent)
    if not root.is_dir():
        raise CatalogError(f"explicit client root is not a directory: {root}")
    layouts: dict[str, Layout] = {}
    for source in SOURCES:
        path = source_path(root, source.relative_path)
        assert_no_reparse(path, root)
        if not path.is_file():
            raise CatalogError(f"missing pinned DAT: {source.relative_path}")
        data = path.read_bytes()
        if len(data) != source.size:
            raise CatalogError(f"{source.relative_path}: size {len(data)} != pinned {source.size}")
        digest = hashlib.sha256(data).hexdigest()
        if digest != source.sha256:
            raise CatalogError(f"{source.relative_path}: SHA-256 {digest} != pinned {source.sha256}")
        layouts[source.key] = parse_layout(source, data)
    return layouts


def timeline_nodes(layout: Layout) -> dict[str, int]:
    result: dict[str, int] = {}
    for node_type in layout.types:
        if node_type.shape != TIME_LINE:
            continue
        for node in node_type.nodes():
            name = layout.payload.string(layout.payload.u32(node + 8))
            if name in result:
                raise CatalogError(f"{layout.source.key}: duplicate TimeLine name {name}")
            result[name] = node
    return result


def owner_for(layout: Layout, timeline: int) -> tuple[int, int, int, str]:
    matches: list[tuple[int, int, int, str]] = []
    for node_type in layout.types:
        if node_type.shape != UNIT_TREE:
            continue
        for owner in node_type.nodes():
            child_ref = layout.payload.u32(owner + 0x30)
            child_count = layout.payload.u32(owner + 0x34)
            children = layout.run(child_ref, child_count, CHILD)
            for child in children:
                if layout.payload.u32(child + 0x18) == timeline:
                    child_name_offset, child_name = layout.string_ref(child, 0x14)
                    matches.append((owner, child, child_name_offset, child_name))
    if len(matches) != 1:
        raise CatalogError(f"{layout.source.key}: expected one direct UnitTree owner for 0x{timeline:X}, got {len(matches)}")
    return matches[0]


def make_records(layouts: dict[str, Layout]) -> list[Record]:
    records: list[Record] = []
    for source in SOURCES:
        layout = layouts[source.key]
        by_name = timeline_nodes(layout)
        names = selected_names(source)
        missing = [name for name in names if name not in by_name]
        if missing:
            raise CatalogError(f"{source.key}: missing allowlisted TimeLines: {', '.join(missing)}")
        for show_name, hide_name in zip(names[::2], names[1::2]):
            show = by_name[show_name]
            hide = by_name[hide_name]
            show_owner, show_child, show_child_name_offset, show_child_name = owner_for(layout, show)
            hide_owner, hide_child, hide_child_name_offset, hide_child_name = owner_for(layout, hide)
            if show_owner != hide_owner:
                raise CatalogError(f"{source.key}: pair owner mismatch {show_name}/{hide_name}")
            for timeline, child, child_name_offset, child_name, side in (
                (show, show_child, show_child_name_offset, show_child_name, "show"),
                (hide, hide_child, hide_child_name_offset, hide_child_name, "hide"),
            ):
                node_type = layout.kind(timeline)
                if node_type.shape != TIME_LINE:
                    raise CatalogError(f"{source.key}: selected node has wrong TimeLine shape")
                name_offset, timeline_name = layout.string_ref(timeline, 8)
                if timeline_name != (show_name if side == "show" else hide_name):
                    raise CatalogError(f"{source.key}: selected name changed during parse")
                property_offset = layout.payload.u32(timeline + 0x1C)
                if layout.kind(property_offset).shape != PROPERTY:
                    raise CatalogError(f"{source.key}: {timeline_name} property reference has an unexpected type")
                layout.payload.span(property_offset, PROPERTY_SIZE)
                property_name_offset, property_name = layout.string_ref(property_offset, 8)
                if property_name != "wprp_0001":
                    raise CatalogError(f"{source.key}: {timeline_name} property name is {property_name!r}")
                vector_offset = layout.payload.u32(property_offset + 0x10)
                vector_count = layout.payload.u32(property_offset + 0x14)
                if vector_count != len(WEATHER_IDS):
                    raise CatalogError(f"{source.key}: {timeline_name} vector count {vector_count} != {len(WEATHER_IDS)}")
                vector_size = vector_count * 8
                layout.run(vector_offset, vector_count, VECTOR_ITEM)
                vector_data = layout.payload.span(vector_offset, vector_size)
                values = tuple(struct.unpack_from("<II", vector_data, i * 8) for i in range(vector_count))
                suffix = timeline_name[len("time_bg_") : -len("_" + side)]
                family = "winter" if suffix in WINTER_PAIRS.get(source.key, ()) else source.family
                role = role_for(family, side)
                if values != expected_vector_fixed(role):
                    raise CatalogError(f"{source.key}: {timeline_name} vector does not match {role}")
                owner_name_offset, owner_name = layout.string_ref(show_owner, 8)
                if show_owner != hide_owner:
                    raise CatalogError(f"{source.key}: owner changed while reading pair")
                child_type = layout.kind(child)
                owner_type = layout.kind(show_owner)
                if owner_type.shape != UNIT_TREE or child_type.shape != CHILD or layout.payload.u32(child + 0x18) != timeline:
                    raise CatalogError(f"{source.key}: {timeline_name} is not a direct owner child")
                scb_offset = layout.payload.u32(timeline + 0x14)
                scb_size = layout.payload.u32(timeline + 0x18)
                if scb_offset == 0 or scb_size == 0:
                    raise CatalogError(f"{source.key}: {timeline_name} has an empty SCB span")
                scb = layout.payload.span(scb_offset, scb_size)
                if scb[:8] != SCB_MAGIC:
                    raise CatalogError(f"{source.key}: {timeline_name} SCB magic mismatch")
                if layout.payload.u16(scb_offset + 0x0E) != 0x30 or layout.payload.u32(scb_offset + 0x10) != scb_size:
                    raise CatalogError(f"{source.key}: {timeline_name} SCB header does not match its declared span")
                records.append(
                    Record(
                        source=source,
                        role=role,
                        timeline_offset=timeline,
                        timeline_size=node_type.size,
                        owner_offset=show_owner,
                        owner_size=owner_type.size,
                        owner_child_offset=child,
                        owner_child_size=child_type.size,
                        owner_child_name_offset=child_name_offset,
                        property_offset=property_offset,
                        property_size=PROPERTY_SIZE,
                        vector_offset=vector_offset,
                        vector_size=vector_size,
                        vector_count=vector_count,
                        name_offset=name_offset,
                        owner_name_offset=owner_name_offset,
                        property_name_offset=property_name_offset,
                        scb_offset=scb_offset,
                        scb_size=scb_size,
                        timeline_name=timeline_name,
                        owner_name=owner_name,
                        owner_child_name=child_name,
                        property_name=property_name,
                        scb_sha256=hashlib.sha256(scb).hexdigest(),
                        scb_fingerprint=fnv1a64(scb),
                    )
                )
    if len(records) != 130:
        raise CatalogError(f"allowlist produced {len(records)} records; expected 130")
    return records


def cpp_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=True)


def render_inc(records: Iterable[Record]) -> str:
    rows = list(records)
    lines = [
        "// Generated by client/tools/build-seasonal-catalog.py. Do not edit.",
        "// Metadata only: no retail DAT or SCB bytes are embedded.",
        "constexpr LayoutInput kSeasonalInputs[] = {",
    ]
    for source in SOURCES:
        lines.append(
            "    { "
            + f"{cpp_string(source.relative_path)}, {source.size}ull, {cpp_string(source.sha256)} }},"
        )
    lines.extend(
        [
            "};",
            "",
        "constexpr TimelineDescriptor kSeasonalCatalog[] = {",
        ]
    )
    for record in rows:
        lines.append(
            "    { "
            + f"{record.source.root}u, 0x{record.source.resource_id:08X}u, TimelineRole::{record.role}, "
            + f"0x{record.timeline_offset:X}u, {record.timeline_size}u, "
            + f"0x{record.owner_offset:X}u, {record.owner_size}u, "
            + f"0x{record.owner_child_offset:X}u, {record.owner_child_size}u, 0x{record.owner_child_name_offset:X}u, "
            + f"0x{record.property_offset:X}u, {record.property_size}u, "
            + f"0x{record.vector_offset:X}u, {record.vector_size}u, {record.vector_count}u, "
            + f"0x{record.name_offset:X}u, 0x{record.owner_name_offset:X}u, 0x{record.property_name_offset:X}u, "
            + f"0x{record.scb_offset:X}u, {record.scb_size}u, 0x{record.scb_fingerprint:016X}ull, "
            + f"{cpp_string(record.timeline_name)}, {cpp_string(record.owner_name)}, {cpp_string(record.owner_child_name)}, {cpp_string(record.property_name)}, "
            + f"{cpp_string(record.scb_sha256)}, {cpp_string(record.source.sha256)}, {cpp_string(record.source.relative_path)} }},"
        )
    lines.append("};")
    lines.append("")
    return "\n".join(lines)


def write_catalog(output: Path, records: list[Record]) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(render_inc(records), encoding="ascii", newline="\n")


def build_catalog(client_root: Path) -> list[Record]:
    return make_records(load_sources(client_root))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--client-root", required=True, type=Path, help="explicit retail client install directory")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "src" / "seasonal_catalog.inc",
        help="generated metadata include path (default: client/src/seasonal_catalog.inc)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare generated metadata with --output without writing it",
    )
    args = parser.parse_args(argv)
    try:
        records = build_catalog(args.client_root)
        if args.check:
            expected = render_inc(records)
            actual = args.output.read_text(encoding="ascii")
            if actual != expected:
                raise CatalogError(f"{args.output}: generated metadata differs")
        else:
            write_catalog(args.output, records)
    except (CatalogError, OSError, struct.error) as error:
        print(f"seasonal catalog: {error}", file=sys.stderr)
        return 2
    action = "checked" if args.check else "wrote"
    print(f"seasonal catalog: {action} {len(records)} records at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
