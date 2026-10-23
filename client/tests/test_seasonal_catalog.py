"""Headless checks for the seasonal catalog parser and metadata."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "build-seasonal-catalog.py"
GENERATED = Path(__file__).resolve().parents[1] / "src" / "seasonal_catalog.inc"


def load_generator():
    spec = importlib.util.spec_from_file_location("seasonal_catalog_generator", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"cannot load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def put_u16(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<H", data, offset, value)


def put_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value)


def put_string(data: bytearray, offset: int, value: str) -> None:
    encoded = value.encode("ascii") + b"\0"
    data[offset : offset + len(encoded)] = encoded


def synthetic_layout(module):
    """Build one small typed LYB with a direct UnitTree owner relation."""

    scene = 0x40
    header_size = 0x14
    total_size = 0x500
    payload_file_offset = scene + header_size
    data = bytearray(scene + total_size)
    data[0:24] = module.MAP_MAGIC
    put_u32(data, 0x20, scene)
    put_u32(data, 0x24, 0)
    data[scene : scene + 8] = module.LYB_MAGIC
    put_u16(data, scene + 0x0E, header_size)
    put_u32(data, scene + 0x10, total_size)

    def poff(offset: int) -> int:
        return payload_file_offset + offset

    put_string(data, poff(0), "lyb")
    put_u32(data, poff(4), total_size)
    type_table = 0x80
    object_list = 0x20
    type_table_end = 0xE0
    put_u32(data, poff(0x10), type_table)
    put_u32(data, poff(0x14), object_list)

    class_node = 0x200
    property_node = 0x220
    vector_node = 0x240
    child_node = 0x300
    owner_node = 0x340
    timeline_node = 0x3A0
    put_u32(data, poff(object_list), timeline_node)
    put_u32(data, poff(object_list + 4), property_node)

    shape_specs = (
        (module.CLASS_NODE, class_node, 1),
        (module.PROPERTY, property_node, 1),
        (module.VECTOR_ITEM, vector_node, 23),
        (module.CHILD, child_node, 1),
        (module.UNIT_TREE, owner_node, 1),
        (module.TIME_LINE, timeline_node, 1),
    )
    field_cursor = type_table_end
    extent_cursor = 0x160
    for index, (shape, start, count) in enumerate(shape_specs):
        extent_ref = extent_cursor + index * 0x10
        fields_ref = field_cursor
        fields = shape[1]
        put_u32(data, poff(extent_ref), start)
        put_u32(data, poff(extent_ref + 4), count)
        put_u32(data, poff(extent_ref + 8), shape[0])
        row = type_table + index * 16
        put_u32(data, poff(row), extent_ref)
        put_u32(data, poff(row + 4), fields_ref)
        put_u32(data, poff(row + 8), 1)
        put_u32(data, poff(row + 12), len(fields))
        for field_offset, field_kind in fields:
            put_u16(data, poff(field_cursor), field_offset)
            put_u16(data, poff(field_cursor + 2), field_kind)
            field_cursor += 4

    put_u32(data, poff(class_node + 8), 0x400)
    put_u32(data, poff(property_node + 8), 0x420)
    put_u32(data, poff(property_node + 0x10), vector_node)
    put_u32(data, poff(property_node + 0x14), 23)
    put_u32(data, poff(child_node + 0x14), 0x4A0)
    put_u32(data, poff(child_node + 0x18), timeline_node)
    put_u32(data, poff(owner_node + 8), 0x490)
    put_u32(data, poff(owner_node + 0x30), child_node)
    put_u32(data, poff(owner_node + 0x34), 1)
    put_u32(data, poff(timeline_node + 8), 0x450)
    put_u32(data, poff(timeline_node + 0x1C), property_node)
    expected_vector = module.expected_vector_fixed("HalloweenShow")
    for index, (value, complement) in enumerate(expected_vector):
        put_u32(data, poff(vector_node + index * 8), value)
        put_u32(data, poff(vector_node + index * 8 + 4), complement)
    put_string(data, poff(0x400), "fixture_class")
    put_string(data, poff(0x420), "wprp_0001")
    put_string(data, poff(0x450), "time_bg_fixture_show")
    put_string(data, poff(0x490), "fixture_owner")
    put_string(data, poff(0x4A0), "fixture_child")

    source = module.Source(
        "synthetic",
        0xF00D0001,
        101,
        "fixture.DAT",
        len(data),
        "0" * 64,
        ("fixture",),
        "public_halloween",
    )
    return source, bytes(data), {
        "payload_file_offset": payload_file_offset,
        "class_node": class_node,
        "timeline": timeline_node,
        "property": property_node,
        "vector": vector_node,
        "owner": owner_node,
        "child": child_node,
        "child_name": 0x4A0,
    }


def expect_catalog_error(module, callback) -> None:
    try:
        callback()
    except module.CatalogError:
        return
    raise AssertionError("expected CatalogError")


def test_synthetic_parser(module) -> None:
    source, data, expected = synthetic_layout(module)
    layout = module.parse_layout(source, data)
    assert layout.payload_file_offset == expected["payload_file_offset"]
    assert layout.class_paths == {expected["class_node"]: "fixture_class"}
    assert module.timeline_nodes(layout) == {"time_bg_fixture_show": expected["timeline"]}
    assert layout.kind(expected["property"]).shape == module.PROPERTY
    assert layout.kind(expected["vector"]).shape == module.VECTOR_ITEM
    assert layout.kind(expected["child"]).shape == module.CHILD
    assert layout.kind(expected["owner"]).shape == module.UNIT_TREE
    assert layout.kind(expected["timeline"]).shape == module.TIME_LINE
    assert len(layout.run(expected["vector"], 23, module.VECTOR_ITEM)) == 23
    owner, child, child_name_offset, child_name = module.owner_for(layout, expected["timeline"])
    assert (owner, child, child_name_offset, child_name) == (
        expected["owner"],
        expected["child"],
        expected["child_name"],
        "fixture_child",
    )
    expect_catalog_error(module, lambda: layout.run(expected["vector"], 23, module.PROPERTY))
    expect_catalog_error(
        module,
        lambda: layout.payload.span(len(layout.payload.data) - 1, 2),
    )

    bad_header = bytearray(data)
    put_u32(bad_header, 0x40 + 0x10, 0x501)
    expect_catalog_error(module, lambda: module.parse_layout(source, bytes(bad_header)))

    bad_tables = bytearray(data)
    put_u32(bad_tables, expected["payload_file_offset"] + 0x10, 0x1C)
    expect_catalog_error(module, lambda: module.parse_layout(source, bytes(bad_tables)))

    bad_owner = bytearray(data)
    put_u32(
        bad_owner,
        expected["payload_file_offset"] + expected["child"] + 0x18,
        expected["timeline"] + 4,
    )
    malformed_layout = module.parse_layout(source, bytes(bad_owner))
    expect_catalog_error(module, lambda: module.owner_for(malformed_layout, expected["timeline"]))


def test_metadata_contract(module) -> None:
    expected_inputs = {
        "public_limsa": (
            "data/29/D9/00/01.DAT",
            1197104,
            "bc40d7ff57ab8998978532cce4564e2488ed81d6d10b8f7ede3e1586b0b26523",
        ),
        "public_gridania": (
            "data/29/B0/00/01.DAT",
            1300928,
            "abc13bd6f096b0fe9703b2092ca4a7f819ff2b88e4d08ae222a9db447f618312",
        ),
        "public_uldah": (
            "data/61/5A/00/01.DAT",
            1036816,
            "69c6ff8488ed9c407a28705cf6cb844d55845f7eed3ad58b31845b11a8331c4c",
        ),
        "interior_limsa": (
            "data/29/D9/00/02.DAT",
            728976,
            "c6d86bfea2692e612464e7e08f6604df4f26fc1ea99da16727e824c920a693d8",
        ),
        "interior_gridania": (
            "data/29/B0/00/02.DAT",
            460368,
            "f2088d1feba5fe71fbc4c462229f1bd1970371d6f023622a7a83d6aa8d450876",
        ),
        "interior_uldah": (
            "data/61/5A/00/03.DAT",
            529616,
            "d969e982a55d5c986e7bea24ae5af1bf23bfe7ed18854f7dd7c4f21a11d2353f",
        ),
    }
    assert {
        source.key: (source.relative_path, source.size, source.sha256)
        for source in module.SOURCES
    } == expected_inputs
    record_fields = {field.name for field in module.Record.__dataclass_fields__.values()}
    assert {
        "timeline_offset",
        "owner_offset",
        "owner_child_name_offset",
        "property_offset",
        "vector_offset",
        "name_offset",
        "scb_offset",
        "scb_fingerprint",
    } <= record_fields

    for role in ("HalloweenShow", "HalloweenHide", "WinterShow", "WinterHide"):
        values = module.expected_vector_fixed(role)
        assert len(values) == 23
        selected = {
            "HalloweenShow": {8027},
            "HalloweenHide": set(module.WEATHER_IDS) - {8027},
            "WinterShow": set(),
            "WinterHide": set(module.WEATHER_IDS),
        }[role]
        for weather_id, value in zip(module.WEATHER_IDS, values):
            if weather_id in selected:
                assert value == (weather_id | 0xFFFF0000, 0xFFFFFFFF)
            else:
                assert value == (weather_id, 0)

    generated = GENERATED.read_text(encoding="ascii")
    assert generated.startswith("// Generated by client/tools/build-seasonal-catalog.py.")
    assert "Metadata only: no retail DAT or SCB bytes are embedded." in generated
    assert "constexpr LayoutInput kSeasonalInputs[]" in generated
    assert "constexpr TimelineDescriptor kSeasonalCatalog[]" in generated
    assert generated.count("TimelineRole::") == 130
    assert "SEDBSCB" not in generated


def test_exact_stock_records(module, client_root: Path) -> None:
    records = module.build_catalog(client_root)
    assert len(records) == 130

    source_counts = {}
    role_counts = {}
    names = {}
    for record in records:
        source_counts[record.source.key] = source_counts.get(record.source.key, 0) + 1
        role_counts[record.role] = role_counts.get(record.role, 0) + 1
        names.setdefault(record.source.key, set()).add(record.timeline_name)
        assert record.vector_count == 23
        assert record.vector_size == 23 * 8
        assert record.owner_child_name_offset > 0
        assert len(record.scb_sha256) == 64
        assert len(record.source.sha256) == 64
        assert record.scb_size > 0
        assert record.scb_fingerprint != 0

    assert source_counts == {
        "public_limsa": 16,
        "public_gridania": 22,
        "public_uldah": 20,
        "interior_limsa": 32,
        "interior_gridania": 22,
        "interior_uldah": 18,
    }
    assert role_counts == {
        "HalloweenShow": 60,
        "HalloweenHide": 60,
        "WinterShow": 5,
        "WinterHide": 5,
    }
    assert all(len(values) == source_counts[key] for key, values in names.items())
    assert GENERATED.read_text(encoding="ascii") == module.render_inc(records)


def test_source_root_is_required(module) -> None:
    with tempfile.TemporaryDirectory(prefix="seasonal-catalog-test-") as temporary:
        output = Path(temporary) / "catalog.inc"
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--output", str(output)],
            capture_output=True,
            text=True,
            check=False,
        )
    assert result.returncode != 0
    assert "required" in result.stderr
    assert not output.exists()


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--client-root",
        type=Path,
        help="optional explicit client root for pinned DAT verification",
    )
    args = parser.parse_args(argv)
    module = load_generator()
    test_synthetic_parser(module)
    test_metadata_contract(module)
    test_source_root_is_required(module)
    if args.client_root is not None:
        test_exact_stock_records(module, args.client_root)
        print(
            "seasonal catalog python tests passed: synthetic parser, metadata, "
            "and pinned stock verification"
        )
    else:
        print(
            "seasonal catalog python tests passed: synthetic parser, metadata, "
            "and explicit-root guard"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
