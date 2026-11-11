#!/usr/bin/env python3
"""Builds the CJK test subset in ../fonts from Noto Sans SC (SIL OFL 1.1).

Usage: subset_cjk.py <NotoSansSC[wght].ttf> <output dir>

Download the source from
https://github.com/google/fonts/raw/main/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf
(license: https://github.com/google/fonts/raw/main/ofl/notosanssc/OFL.txt). Needs fontTools.

The subset: GB2312 level 1 (the 3755 most common hanzi), CJK symbols and punctuation, kana,
fullwidth forms and general punctuation, at weights 400 and 600, without hinting.
"""

import os
import subprocess
import sys
import tempfile

from fontTools.ttLib import TTFont


def chars():
    out = set()
    for b1 in range(0xB0, 0xD8):
        for b2 in range(0xA1, 0xFF):
            try:
                out.add(bytes([b1, b2]).decode("gb2312"))
            except UnicodeDecodeError:
                pass
    for lo, hi in [(0x3000, 0x303F), (0x3040, 0x30FF), (0xFF00, 0xFFEF), (0x2010, 0x2027), (0x2030, 0x203B)]:
        out.update(chr(c) for c in range(lo, hi + 1))
    out.add("·")
    return "".join(sorted(out))


WEIGHTS = {400: "Regular", 600: "SemiBold"}


def main():
    source, out_dir = sys.argv[1], sys.argv[2]
    with tempfile.TemporaryDirectory() as tmp:
        text = os.path.join(tmp, "chars.txt")
        with open(text, "w") as f:
            f.write(chars())
        for weight, style in WEIGHTS.items():
            inst = os.path.join(tmp, f"inst-{weight}.ttf")
            subprocess.run(["fonttools", "varLib.instancer", source, f"wght={weight}", "-o", inst, "-q", "--update-name-table"], check=True)
            out = os.path.join(out_dir, f"NotoSansSC-{style}-subset.ttf")
            subprocess.run(["pyftsubset", inst, f"--text-file={text}", "--no-hinting", "--desubroutinize", f"--output-file={out}"], check=True)
            # One family with two weights: the typographic family and subfamily names.
            font = TTFont(out)
            name = font["name"]
            for rec in list(name.names):
                if rec.nameID in (16, 17):
                    name.removeNames(nameID=rec.nameID)
            name.setName("Noto Sans SC", 16, 3, 1, 0x409)
            name.setName(style, 17, 3, 1, 0x409)
            font.save(out)
            print(out, os.path.getsize(out))


if __name__ == "__main__":
    main()
