import hashlib
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "targetlines_probe_reader", Path(__file__).resolve().parents[1] / "tools/read-targetlines-probe.py"
)
READER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(READER)


def record(kind, sequence, payload=b"", generation=0):
    return struct.pack("<IIQqIII", kind, len(payload), sequence, 123, 456, 0, generation) + payload


def header(magic=b"BTLP0001"):
    return struct.pack("<8s65sQ16sq", magic, READER.CLIENT_SHA256.encode(),
                       15996808, b"2012.09.19.0001", 10000000)


class TargetlinesProbeReaderTests(unittest.TestCase):
    def summarize(self, data):
        with tempfile.TemporaryDirectory(prefix="bahamut-targetlines-reader-") as directory:
            capture = Path(directory) / "capture.bin"
            capture.write_bytes(data)
            return READER.summarize(capture)

    def test_copied_draw_layout_and_camera_link(self):
        camera = struct.pack("<Q64f", 1, *([0.0] * 64))
        shader = b"\x00\x03\xfe\xff\xff\xff\x00\x00"
        draw = struct.pack(
            "<Q5I2f10I6f", 1, 99, 20, 30, 800, 600, 0.0, 1.0,
            800, 600, 1600, 1200, 4, len(shader), 11, 22, 130, 1,
            0.0, 10.0, 0.0, 3.0, 20.0, 4.0,
        ) + struct.pack("<16f", *([0.0] * 16)) + shader
        summary = self.summarize(header() + record(1, 1, camera) + record(2, 2, draw) + record(3, 3))
        self.assertEqual(summary["draw_records"], 1)
        self.assertEqual(summary["positioned_draws"], 1)
        self.assertEqual(summary["zones"], [130])
        self.assertEqual(summary["viewports"][0]["values"], [20, 30, 800, 600, 0.0, 1.0])
        self.assertEqual(summary["surfaces"][0]["values"], [99, 800, 600, 1600, 1200])
        self.assertIsNone(summary["surfaces"][0]["is_back_buffer"])
        self.assertIsNone(summary["back_buffer_draws"])
        self.assertFalse(summary["incomplete_tail"])

    def test_v2_distinguishes_same_size_render_surfaces(self):
        camera = struct.pack("<Q64f", 1, *([0.0] * 64))
        shader = b"\x00\x03\xfe\xff\xff\xff\x00\x00"
        draws = []
        for surface, is_back_buffer in ((101, 1), (102, 0)):
            metadata = struct.pack(
                "<Q5I2f10I6fI", 1, surface, 0, 0, 1600, 1200, 0.0, 1.0,
                1600, 1200, 1600, 1200, 4, len(shader), 11, 22, 128, 1,
                0.0, 10.0, 0.0, 3.0, 20.0, 4.0, is_back_buffer,
            )
            draws.append(record(2, surface, metadata + bytes(4 * 16) + shader))
        summary = self.summarize(header(b"BTLP0002") + record(1, 1, camera)
                                 + b"".join(draws))
        self.assertEqual(summary["format_version"], 2)
        self.assertEqual(summary["draw_records"], 2)
        self.assertEqual(summary["back_buffer_draws"], 1)
        self.assertEqual([item["is_back_buffer"] for item in summary["surfaces"]], [True, False])
        self.assertEqual(summary["shader_sha256"], {hashlib.sha256(shader).hexdigest(): 2})

    def test_v2_invalid_back_buffer_comparison_is_rejected(self):
        camera = struct.pack("<Q64f", 1, *([0.0] * 64))
        draw = struct.pack("<Q5I2f10I6fI", 1, *([0] * 5), 0.0, 1.0,
                           800, 600, 800, 600, 4, 4, 0, 0, 0, 0, *([0.0] * 6), 2)
        draw += bytes(4 * 16 + 4)
        with self.assertRaisesRegex(ValueError, "invalid back-buffer comparison"):
            self.summarize(header(b"BTLP0002") + record(1, 1, camera) + record(2, 2, draw))

    def test_reset_rejects_camera_from_old_generation(self):
        camera = struct.pack("<Q64f", 1, *([0.0] * 64))
        draw = struct.pack("<Q5I2f10I6f", 1, *([0] * 5), 0.0, 1.0,
                           800, 600, 800, 600, 4, 4, 0, 0, 0, 0, *([0.0] * 6))
        draw += bytes(4 * 16 + 4)
        with self.assertRaisesRegex(ValueError, "missing camera record"):
            self.summarize(header() + record(1, 1, camera) + record(4, 2, generation=1)
                           + record(2, 3, draw, generation=1))

    def test_truncated_tail_preserves_complete_records(self):
        summary = self.summarize(header() + record(3, 1) + record(1, 2, bytes(264))[:40])
        self.assertEqual(summary["present_records"], 1)
        self.assertTrue(summary["incomplete_tail"])

    def test_other_client_identity_is_rejected(self):
        data = bytearray(header())
        data[8] = ord("0")
        with self.assertRaisesRegex(ValueError, "client identity"):
            self.summarize(data)


if __name__ == "__main__":
    unittest.main()
