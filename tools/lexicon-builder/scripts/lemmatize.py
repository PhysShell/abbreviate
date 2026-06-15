#!/usr/bin/env python3
"""Lexicon pipeline step: fill the lemma and grammeme columns of an engine
TSV (`form<TAB>lemma<TAB>freq` -> `form<TAB>lemma<TAB>freq<TAB>tags`).

Uses pymorphy3 (the maintained pymorphy2 fork; OpenCorpora dictionaries,
MIT) and takes the most probable parse per surface form; the grammeme tag
of that parse (e.g. `NOUN,inan,femn,sing,loct`) becomes the 4th column,
consumed by abbrev_core::morph for case agreement. Offline tooling only —
never runs on device. Idempotent: re-running on a 4-column file is fine.

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
            parts = line.split("\t")
            form, lemma, freq = parts[0], parts[1], parts[2]
            parse = morph.parse(form)[0]
            new_lemma = parse.normal_form
            # Tag as a comma-joined grammeme string (abbrev_core::morph splits
            # on commas and spaces, so pymorphy's "NOUN,inan femn,sing,loct" is
            # fine); tabs/newlines can't occur in grammeme tags.
            tags = str(parse.tag)
            if new_lemma != lemma:
                changed += 1
            out_lines.append(f"{form}\t{new_lemma}\t{freq}\t{tags}")
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
