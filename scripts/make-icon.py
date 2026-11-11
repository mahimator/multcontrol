#!/usr/bin/env python3
"""Generate the multcontrol app icon.

Draws a simple monitor-and-keyboard glyph as an RGBA PNG, then emits the
sizes macOS (.iconset) and Windows (.ico) want. No third-party libraries:
the PNG is written by hand with zlib + struct.
"""

import math
import os
import struct
import sys
import zlib


def write_png(path, width, height, pixels):
    """Write RGBA pixel data (bytes, row-major) as a PNG file."""

    def chunk(tag, data):
        out = struct.pack(">I", len(data)) + tag + data
        return out + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = bytearray()
    stride = width * 4
    for y in range(height):
        raw.append(0)  # filter type 0
        raw.extend(pixels[y * stride:(y + 1) * stride])

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    blob = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as handle:
        handle.write(blob)


def blend(dst, src, alpha):
    """Source-over blend of an RGBA tuple onto an RGBA tuple."""
    sr, sg, sb, sa = src
    dr, dg, db, da = dst
    sa *= alpha
    out_a = sa + da * (1 - sa)
    if out_a <= 0:
        return (0, 0, 0, 0)
    out_r = (sr * sa + dr * da * (1 - sa)) / out_a
    out_g = (sg * sa + dg * da * (1 - sa)) / out_a
    out_b = (sb * sa + db * da * (1 - sa)) / out_a
    return (out_r, out_g, out_b, out_a)


def rounded_rect(px, py, x0, y0, x1, y1, radius):
    """Signed-distance-ish test for a rounded rectangle."""
    cx = min(max(px, x0 + radius), x1 - radius)
    cy = min(max(py, y0 + radius), y1 - radius)
    return math.hypot(px - cx, py - cy) <= radius


def point_in_polygon(px, py, points):
    """Even-odd point-in-polygon test."""
    inside = False
    count = len(points)
    for i in range(count):
        x0, y0 = points[i]
        x1, y1 = points[(i + 1) % count]
        if (y0 > py) != (y1 > py):
            t = (py - y0) / (y1 - y0)
            if px < x0 + t * (x1 - x0):
                inside = not inside
    return inside


def render(size):
    """Render the icon at `size` pixels square, returning RGBA bytes."""
    s = float(size)
    pixels = bytearray(size * size * 4)

    # Palette: a deep blue rounded square with a white screen + cursor.
    bg = (36, 62, 120, 1.0)
    screen = (245, 248, 255, 1.0)
    accent = (58, 150, 235, 1.0)

    # Geometry, in unit coordinates.
    pad = 0.08 * s
    box0 = pad
    box1 = s - pad
    radius = 0.16 * s

    # Screen area inside the rounded square.
    scr_x0 = box0 + 0.13 * s
    scr_y0 = box0 + 0.15 * s
    scr_x1 = box1 - 0.13 * s
    scr_y1 = box1 - 0.30 * s
    scr_radius = 0.045 * s

    # Stand.
    stand_w = 0.055 * s
    stand_x0 = s / 2 - stand_w
    stand_x1 = s / 2 + stand_w
    stand_y0 = scr_y1 + 0.03 * s
    stand_y1 = box1 - 0.10 * s
    base_x0 = s / 2 - 0.17 * s
    base_x1 = s / 2 + 0.17 * s
    base_y0 = box1 - 0.105 * s
    base_y1 = box1 - 0.055 * s

    # Classic mouse-pointer arrow, inside the screen.
    aw = (scr_x1 - scr_x0) * 0.30
    ah = (scr_y1 - scr_y0) * 0.52
    ax = scr_x0 + (scr_x1 - scr_x0) * 0.26
    ay = scr_y0 + (scr_y1 - scr_y0) * 0.20
    arrow = [
        (ax, ay),
        (ax, ay + ah),
        (ax + aw * 0.30, ay + ah * 0.74),
        (ax + aw * 0.52, ay + ah * 1.00),
        (ax + aw * 0.72, ay + ah * 0.86),
        (ax + aw * 0.50, ay + ah * 0.60),
        (ax + aw * 0.86, ay + ah * 0.60),
    ]

    for y in range(size):
        for x in range(size):
            px = x + 0.5
            py = y + 0.5
            color = (0, 0, 0, 0.0)

            if rounded_rect(px, py, box0, box0, box1, box1, radius):
                color = bg

                # Screen.
                if rounded_rect(px, py, scr_x0, scr_y0, scr_x1, scr_y1, scr_radius):
                    color = screen

                    # Cursor arrow.
                    if point_in_polygon(px, py, arrow):
                        color = accent

                # Stand and base.
                elif stand_x0 <= px <= stand_x1 and stand_y0 <= py <= stand_y1:
                    color = screen
                elif base_x0 <= px <= base_x1 and base_y0 <= py <= base_y1:
                    color = screen

            idx = (y * size + x) * 4
            pixels[idx:idx + 4] = bytes(
                (
                    int(color[0]),
                    int(color[1]),
                    int(color[2]),
                    int(color[3] * 255),
                )
            )
    return bytes(pixels)


def write_ico(path, sizes):
    """Write a multi-resolution .ico file (PNG-compressed entries)."""
    images = []
    for size in sizes:
        pixels = render(size)
        images.append((size, pixels))

    count = len(images)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count

    entries = bytearray()
    blobs = bytearray()
    for size, pixels in images:
        # Each image is stored as a PNG inside the .ico container.
        tmp = f"{path}.tmp{size}.png"
        write_png(tmp, size, size, pixels)
        with open(tmp, "rb") as handle:
            blob = handle.read()
        os.remove(tmp)
        entries += struct.pack(
            "<BBBBHHII",
            size if size < 256 else 0,
            size if size < 256 else 0,
            0,
            0,
            1,
            32,
            len(blob),
            offset,
        )
        blobs += blob
        offset += len(blob)

    with open(path, "wb") as handle:
        handle.write(header + bytes(entries) + bytes(blobs))


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "assets"
    os.makedirs(out_dir, exist_ok=True)

    # macOS .iconset
    iconset = os.path.join(out_dir, "MultControl.iconset")
    os.makedirs(iconset, exist_ok=True)
    mac_sizes = {
        "icon_16x16": 16,
        "icon_16x16@2x": 32,
        "icon_32x32": 32,
        "icon_32x32@2x": 64,
        "icon_128x128": 128,
        "icon_128x128@2x": 256,
        "icon_256x256": 256,
        "icon_256x256@2x": 512,
        "icon_512x512": 512,
        "icon_512x512@2x": 1024,
    }
    for name, size in mac_sizes.items():
        write_png(
            os.path.join(iconset, f"{name}.png"), size, size, render(size)
        )
        print(f"  {name}.png ({size}x{size})")

    # Windows .ico
    write_ico(
        os.path.join(out_dir, "multcontrol.ico"),
        [16, 24, 32, 48, 64, 128, 256],
    )
    print("  multcontrol.ico")

    # A plain 512px PNG for other uses (README, docs).
    write_png(os.path.join(out_dir, "multcontrol.png"), 512, 512, render(512))
    print("  multcontrol.png")


if __name__ == "__main__":
    main()
