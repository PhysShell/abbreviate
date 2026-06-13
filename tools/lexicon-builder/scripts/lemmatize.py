#!/usr/bin/env python3
"""Lexicon pipeline step: fill the lemma column of an engine TSV.

Uses pymorphy3 (the maintained pymorphy2 fork; OpenCorpora dictionaries,
MIT) and takes the most probable parse per surface form. Offline tooling
only — never runs on device.

    pip install pymorphy3 pymorphy3-dicts-ru
    python3 lemmatize.py data/lexicons/ru-50k.tsv
"""

import os
import sys
import tempfile


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: lemmatize.py <lexicon.tsv>", file=sys.stderr)
        return 1
    path = sys.argv[1]
    # Validate the input before the heavy import: a bad path should be
    # reported as such, not as a missing dependency.
    try:
        probe = open(path, encoding="utf-8")
    except OSError as e:
        print(f"cannot read {path}: {e}", file=sys.stderr)
        return 1
    probe.close()
    # Lazy import: usage and argument errors must not require the
    # dependency to be installed.
    try:
        import pymorphy3
    except ModuleNotFoundError:
        print(
            "pymorphy3 is not installed; run: pip install pymorphy3 pymorphy3-dicts-ru",
            file=sys.stderr,
        )
        return 1
    morph = pymorphy3.MorphAnalyzer()
    out_lines = []
    changed = 0
    with open(path, encoding="utf-8") as f:
        for raw in f:
            line = raw.rstrip("\n")
            if not line or line.startswith("#"):
                out_lines.append(line)
                continue
            form, lemma, freq = line.split("\t")
            new_lemma = morph.parse(form)[0].normal_form
            if new_lemma != lemma:
                changed += 1
            out_lines.append(f"{form}\t{new_lemma}\t{freq}")
    # Atomic replace: a crash mid-write must not corrupt the artifact.
    fd, tmp_path = tempfile.mkstemp(dir=os.path.dirname(path) or ".", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write("\n".join(out_lines) + "\n")
        os.replace(tmp_path, path)
    except BaseException:
        os.unlink(tmp_path)
        raise
    print(f"lemmatized {path}: {changed} lemmas updated", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
