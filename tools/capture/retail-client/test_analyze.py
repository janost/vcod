"""analyze.py's readers: python3 -m unittest discover -s tools/capture/retail-client"""

import os
import struct
import tempfile
import unittest

import analyze


def write_tga(path, w, h, pixel, top_down=False, rle=False):
    """A 24-bit TGA of pixel(x, y) -> (r, g, b), y counted from the top."""
    rows = []
    for y in range(h):
        rows.append(b"".join(bytes((b, g, r)) for r, g, b in (pixel(x, y) for x in range(w))))
    if not top_down:
        rows.reverse()
    body = b"".join(rows)
    if rle:
        # One raw packet per pixel: valid RLE, and exercises the reader's path.
        body = b"".join(b"\x00" + body[i:i + 3] for i in range(0, len(body), 3))
    head = struct.pack("<BBBHHBHHHHBB", 0, 0, 10 if rle else 2, 0, 0, 0, 0, 0, w, h, 24,
                       0x20 if top_down else 0)
    with open(path, "wb") as f:
        f.write(head + body)


def log_lines(text):
    return [analyze.fields(l) for l in text.strip().splitlines()]


class ClientLog(unittest.TestCase):
    def test_a_packet_dup_cmd_is_read_once(self):
        log = log_lines("""
enter w=1 svs=1 slot=0 proto=1 addr=x name=a b
move w=1 svs=1 slot=0 seq=1 ack=0 delta=1 n=2
cmd slot=0 st=100 b=0 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
cmd slot=0 st=110 b=0 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
move w=2 svs=2 slot=0 seq=2 ack=0 delta=1 n=2
cmd slot=0 st=110 b=0 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
cmd slot=0 st=120 b=1 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
""")
        self.assertEqual([c["st"] for c in analyze.cmd_stream(log, 0)], [100, 110, 120])
        self.assertEqual(log[0][1]["name"], "a b")

    def test_an_fdc_marks_the_newest_cmd_of_the_packet_after_it(self):
        log = log_lines("""
enter w=1 svs=1 slot=0 proto=22 addr=x name=a
move w=1 svs=1 slot=0 seq=1 ack=0 delta=1 n=2
cmd slot=0 st=100 b=0 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
cmd slot=0 st=108 b=0 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
ccmd w=2 svs=2 slot=0 seq=5 ok=1 text=fdc 1
move w=2 svs=2 slot=0 seq=2 ack=0 delta=1 n=2
cmd slot=0 st=108 b=0 wb=0 w=1 fl=0 a=0,0,0 mv=0,0,0
cmd slot=0 st=116 b=0 wb=1 w=1 fl=0 a=0,0,0 mv=0,0,0
move w=3 svs=3 slot=0 seq=3 ack=0 delta=1 n=2
cmd slot=0 st=116 b=0 wb=1 w=1 fl=0 a=0,0,0 mv=0,0,0
cmd slot=0 st=124 b=0 wb=1 w=1 fl=0 a=0,0,0 mv=0,0,0
""")
        self.assertEqual(analyze.fdc_frames(log), [(116, 1, True)])

    def test_a_shot_s_kick_is_counted_from_its_own_cmd(self):
        lines = ["enter w=1 svs=1 slot=0 proto=1 addr=x name=a"]
        pitch = {0: 0, 1: 0, 2: 0, 3: 0, 4: 0, 5: 0, 6: 0, 7: -60, 8: -100, 9: -90}
        for i, p in pitch.items():
            lines.append(f"move w={i} svs={i} slot=0 seq={i} ack=0 delta=1 n=1")
            lines.append(f"cmd slot=0 st={1000 + 10 * i} b={1 if i == 6 else 0} wb=0 w=1 fl=0 "
                         f"a={p % 65536},5,0 mv=0,0,0")
        lines.append("ran slot=0 st=1060 ev=159:0")
        _, shots = analyze.kick_shots(log_lines("\n".join(lines)), 0)
        self.assertEqual(len(shots), 1)
        s = shots[0]
        self.assertTrue(s["still"])
        self.assertEqual(s["kick"], 1)
        self.assertEqual(s["trail"][1]["d"], [-60, 0])

    def test_the_fixture_keeps_only_shots_whose_view_comes_back(self):
        def take(back):
            lines = ["enter w=1 svs=1 slot=0 proto=1 addr=x name=a"]
            for i in range(40):
                p = 0 if i < 7 else (-8 if i < 12 else (0 if back else -10))
                lines.append(f"move w={i} svs={i} slot=0 seq={i} ack=0 delta=1 n=1")
                lines.append(f"cmd slot=0 st={1000 + i} b={int(i >= 6)} wb=0 w=1 fl=0 "
                             f"a={p % 65536},5,0 mv=0,0,0")
            lines.append("ran slot=0 st=1006 ev=159:0")
            return analyze.kick_fixture(log_lines("\n".join(lines)), 0)
        [block] = take(True)
        self.assertEqual(block[0], "cmd st=1003 b=0 a=0,5,0")
        self.assertEqual(block[4], "ran st=1006 ev=159:0")
        self.assertEqual(block[-1], "cmd st=1031 b=1 a=0,5,0")
        self.assertEqual(take(False), [])

    def test_angle_shorts_wrap(self):
        self.assertEqual(analyze.short_delta(65530, 4), 10)
        self.assertEqual(analyze.short_delta(4, 65530), -10)


class Screenshots(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.dir.cleanup()

    def path(self, name):
        return os.path.join(self.dir.name, name)

    def test_tga_rows_read_top_down_whatever_the_file_order(self):
        for top_down, rle in ((False, False), (True, False), (False, True)):
            p = self.path("t.tga")
            write_tga(p, 4, 2, lambda x, y: (255, 0, 0) if y == 0 else (0, 0, 255), top_down, rle)
            w, h, rows = analyze.read_tga(p)
            self.assertEqual((w, h), (4, 2))
            self.assertEqual(rows[0][:3], b"\xff\x00\x00")
            self.assertEqual(rows[1][:3], b"\x00\x00\xff")

    def test_the_boxes_are_found_on_a_stretched_shot(self):
        # 800x600: virtual (16, 216, 48) is pixels (20, 270, 60).
        def pixel(x, y):
            if 20 <= x < 80 and 270 <= y < 330:
                return (255, 0, 255)
            return (40, 40, 40)
        p = self.path("rc_play_1.tga")
        write_tga(p, 800, 600, pixel)
        self.assertEqual(analyze.boxes_seen(p), {"level": True, "client": False})

    def test_a_wide_icon_reaches_left_of_a_narrow_one(self):
        # 1.1's floor: green grass that shifts between shots, with the item
        # lying on it in the scan; the icons are grey.
        def shot(left, shift, item):
            def pixel(x, y):
                vx, vy = x * 640 / 320, y * 480 / 240
                if left <= vx < 340 and 330 <= vy < 360:
                    return (200, 200, 190)
                if item and 256 <= vx < 262:
                    return (150, 150, 140)
                g = 40 + ((x + shift) * 37 + y * 11) % 90
                return (g, g, g // 2)
            return pixel
        paths = {}
        for name, left, shift, item in (("b", 1000, 0, False), ("n", 300, 3, True),
                                        ("w", 280, 5, True)):
            paths[name] = self.path(name + ".tga")
            write_tga(paths[name], 320, 240, shot(left, shift, item))
        b = analyze.read_tga(paths["b"])
        self.assertEqual(analyze.icon_left(analyze.read_tga(paths["n"]), b), 300)
        self.assertEqual(analyze.icon_left(analyze.read_tga(paths["w"]), b), 280)
        self.assertIsNone(analyze.icon_left(b, b))

if __name__ == "__main__":
    unittest.main()
