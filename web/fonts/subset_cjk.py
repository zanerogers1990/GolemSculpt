"""Makes the web version's Japanese, Chinese and Korean fonts (web/fonts/*.otf).

The tooltips in those languages (crates/toolkit/translations/ja.txt, zh.txt,
ko.txt) need letters the web build's own font (Ubuntu Light) has not got,
and a page cannot use the computer's fonts. These are Noto Sans CJK
DemiLight (SIL Open Font License 1.1), the Japanese, Simplified Chinese and
Korean faces, each cut down to the letters its language's tooltips use, plus
the languages' names and the word "language" in each, so the download stays
small. The page fetches one only when that language is picked (or the
language question or picker shows the names).

Needs fontTools. Run it again when the translations change:
    python web/fonts/subset_cjk.py <path to NotoSansCJK-DemiLight.ttc>
"""
import os
import sys

from fontTools import subset
from fontTools.ttLib import TTCollection

HERE = os.path.dirname(os.path.abspath(__file__))
TRANSLATIONS = os.path.join(HERE, "..", "..", "crates", "toolkit", "translations")
# Face of the collection for each language, and the file the page fetches.
FACES = {"ja": (0, "cjk-ja.otf"), "zh": (2, "cjk-zh.otf"), "ko": (1, "cjk-ko.otf")}
# Every language's name and its word for "language", which the picker and
# the first-run question show in every one of these fonts.
NAMES = "日本語简体中文한국어言語语言언어 · ()"


def letters(code):
    with open(os.path.join(TRANSLATIONS, code + ".txt"), encoding="utf-8") as f:
        text = f.read()
    return sorted(set(c for c in text + NAMES if ord(c) > 0x7F or c.isprintable()))


def main():
    for code, (face, out) in FACES.items():
        # Read afresh for each: the faces of the collection share their
        # outlines, and cutting one down would cut the others too.
        font = TTCollection(sys.argv[1]).fonts[face]
        options = subset.Options()
        options.layout_features = ["*"]
        options.name_IDs = ["*"]
        options.name_languages = ["*"]
        options.notdef_outline = True
        options.hinting = False
        options.desubroutinize = False
        sub = subset.Subsetter(options)
        sub.populate(text="".join(letters(code)))
        sub.subset(font)
        path = os.path.join(HERE, out)
        font.save(path)
        print(out, os.path.getsize(path) // 1024, "KB")


if __name__ == "__main__":
    main()
