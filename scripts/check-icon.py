#!/usr/bin/env python3
"""Verify that a PE file carries an embedded application icon.

Walks the resource tree and counts the RT_ICON entries. Exits non-zero when
the icon is missing, so CI fails loudly instead of shipping an exe without
one.
"""

import struct
import sys


def main(path):
    data = open(path, "rb").read()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0":
        print(f"{path}: not a PE file")
        return 1
    nsec = struct.unpack_from("<H", data, pe + 6)[0]
    optsz = struct.unpack_from("<H", data, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", data, opt)[0]
    ddoff = opt + (112 if magic == 0x20B else 96)
    rva, size = struct.unpack_from("<II", data, ddoff + 2 * 8)
    if rva == 0 or size == 0:
        print(f"{path}: NO resource directory - the icon is missing")
        return 1

    sect = opt + optsz
    sections = []
    base = None
    for i in range(nsec):
        off = sect + i * 40
        va = struct.unpack_from("<I", data, off + 12)[0]
        vsz = struct.unpack_from("<I", data, off + 8)[0]
        praw = struct.unpack_from("<I", data, off + 20)[0]
        sections.append((va, vsz, praw))
        if base is None and va <= rva < va + max(vsz, 1):
            base = praw + (rva - va)
    if base is None:
        print(f"{path}: resource directory not found")
        return 1

    def entries(offset):
        named, ids = struct.unpack_from("<HH", data, offset + 12)
        out = []
        pos = offset + 16
        for _ in range(named + ids):
            name_id, off2 = struct.unpack_from("<II", data, pos)
            out.append((name_id, base + (off2 & 0x7FFFFFFF)))
            pos += 8
        return out

    icons = 0
    groups = 0
    for type_id, type_off in entries(base):
        if type_id == 3:  # RT_ICON
            for _, name_off in entries(type_off):
                for _, lang_off in entries(name_off):
                    leaf_rva, leaf_size, _ = struct.unpack_from("<III", data, lang_off)
                    if leaf_size > 0:
                        icons += 1
        elif type_id == 14:  # RT_GROUP_ICON
            for _, name_off in entries(type_off):
                for _, lang_off in entries(name_off):
                    leaf_rva, leaf_size, _ = struct.unpack_from("<III", data, lang_off)
                    if leaf_size > 0:
                        groups += 1

    if groups == 0 or icons == 0:
        print(f"{path}: icon resources are MISSING (groups={groups} icons={icons})")
        return 1
    print(f"{path}: icon embedded ({groups} group, {icons} images)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
