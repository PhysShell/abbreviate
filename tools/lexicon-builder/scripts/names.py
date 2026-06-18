#!/usr/bin/env python3
"""Lexicon pipeline step: turn plain name lists into engine lexicon rows.

Russian given names, surnames and patronymics decline, so the long tail of
proper names this IME otherwise can't reach (`ивн`->Иван, `пткв`->Петкова) is
best served by folding the *declined surface forms* into the lexicon, exactly
like ordinary words. This reads one name per line and emits the engine TSV
`form<TAB>lemma<TAB>freq<TAB>tags`, ready to merge into the surface-form
lexicon (concatenate the output as data/lexicons/ru-names.tsv; the normal
build sorts and folds it in alongside ru-50k.tsv).

Source of the name lists: the Natasha name dictionaries redistributed in
mawo-nlp-data (first/last/middle, ~113k entries; MIT, (c) Alexander
Kukushkin). Export each `.dict` to a newline-delimited text file and pass it
here. The frequencies НКРЯ also bundled in mawo-nlp-data are NOT used: their
redistribution terms are unstated, so they stay a build-time-only input via
`lexicon-builder rnc`, never versioned (see docs/ARCHITECTURE.md).

Forms are generated with mawo-pymorphy3 (bundled OpenCorpora 2025 DAWG; code
MIT / dicts CC BY-SA 3.0): each name is inflected through the six cases (given
names and patronymics in the singular; surnames also in the family plural --
Ивановы). pymorphy lowercases inflected output, so forms are re-capitalised
for display -- the engine normalises lookup keys itself (alphabet::normalize),
keeping the displayed spelling intact.

Names carry no corpus frequency, so every form gets a flat low prior (--freq,
default 1.0): a real word therefore always outranks a same-spelled name
(`роман` the noun stays above `Роман` the name) while the name stays
retrievable. Offline tooling only -- never runs on device. Deterministic and
idempotent: same inputs -> byte-identical output.

    pip install mawo-pymorphy3        # bundled OpenCorpora 2025 DAWG dicts
    python3 names.py --first first_all_2025.txt --surname last_updated_2025.txt \
        --patronymic middle.txt -o data/lexicons/ru-names.tsv
"""

import argparse
import gzip
import os
import sys
import tempfile

CASES = ["nomn", "gent", "datv", "accs", "ablt", "loct"]

# Per name kind: the pymorphy grammeme that marks it, and whether the family
# plural (Ивановы) is a meaningful form for that kind.
KINDS = {
    "first": ("Name", False),
    "surname": ("Surn", True),
    "patronymic": ("Patr", False),
}


def titlecase(word: str) -> str:
    """Capitalise a (possibly hyphenated) name: Анна-Мария, not Анна-мария.

    str.capitalize() lowercases the tail, which is what we want per segment
    but would mangle the second half of a hyphenated name, so split first."""
    return "-".join(part.capitalize() for part in word.split("-"))


def read_names(path):
    """Read one name per line from a plain or gzipped UTF-8 text list.

    The Natasha `.dict` files this targets are newline-delimited text (the
    legacy yargy-era name lists), so a `.dict`, `.txt` or `.gz` all work: we
    sniff the gzip magic and decode UTF-8. If the bytes aren't decodable text
    (a packed DAWG/marisa-trie or a pickle), fail loudly with guidance instead
    of emitting garbage 'names' — the format assumption lives here, in one
    place, so a surprise is a one-line fix rather than a corrupt artifact."""
    with open(path, "rb") as f:
        blob = f.read()
    if blob[:2] == b"\x1f\x8b":  # gzip magic
        blob = gzip.decompress(blob)
    if b"\x00" in blob:
        raise ValueError(
            f"{path}: binary content (NUL bytes), not a newline name list. "
            "Expected plain/gzipped UTF-8 text, one name per line; if mawo "
            "ships a packed DAWG/pickle, convert it first (see read_names)."
        )
    try:
        text = blob.decode("utf-8")
    except UnicodeDecodeError as e:
        raise ValueError(f"{path}: not UTF-8 text ({e}); see read_names docstring") from e
    names = []
    for raw in text.splitlines():
        name = raw.strip()
        if name and not name.startswith("#"):
            names.append(name)
    return names


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Build engine lexicon rows from Russian name lists."
    )
    ap.add_argument("--first", action="append", default=[], metavar="FILE",
                    help="given-name list (one per line); repeatable")
    ap.add_argument("--surname", action="append", default=[], metavar="FILE",
                    help="surname list; repeatable")
    ap.add_argument("--patronymic", action="append", default=[], metavar="FILE",
                    help="patronymic list; repeatable")
    ap.add_argument("--freq", type=float, default=1.0,
                    help="flat frequency prior per name form (default 1.0)")
    ap.add_argument("-o", "--output", required=True, metavar="FILE")
    args = ap.parse_args()

    jobs = ([(p, "first") for p in args.first]
            + [(p, "surname") for p in args.surname]
            + [(p, "patronymic") for p in args.patronymic])
    if not jobs:
        ap.error("give at least one --first/--surname/--patronymic list")
    if not (args.freq > 0 and args.freq == args.freq):  # finite and positive
        ap.error("--freq must be a finite positive number")

    # Validate inputs before the heavy import: a bad path should be reported
    # as such, not as a missing dependency.
    names = []  # (token, kind)
    for path, kind in jobs:
        try:
            names.extend((token, kind) for token in read_names(path))
        except (OSError, ValueError) as e:
            print(f"cannot read {path}: {e}", file=sys.stderr)
            return 1

    # Lazy import: usage and argument errors must not require the dependency.
    try:
        import mawo_pymorphy3

        morph = mawo_pymorphy3.create_analyzer()
    except ModuleNotFoundError:
        print("mawo-pymorphy3 is not installed; run: pip install mawo-pymorphy3",
              file=sys.stderr)
        return 1

    def pick(parses, grammeme):
        # Only emit forms pymorphy actually tags as this kind of name. An
        # earlier NOUN fallback let unknown tokens through as guessed common
        # nouns (wrong gender/animacy: Ивановаа, Ивановва) and pulled in
        # non-name homographs (Ивановец the resident, Иваново the Geox place,
        # an invented plural lemma Ивановичь) — so drop anything not tagged
        # Name/Surn/Patr rather than inflect a wrong reading.
        return next((p for p in parses if grammeme in p.tag), None)

    # (form, lemma) -> tags. The CASES order puts nomn first, so a syncretic
    # form (Иванова = gent & accs) keeps a single, deterministic tag.
    rows = {}
    skipped = 0
    for token, kind in names:
        grammeme, do_plur = KINDS[kind]
        parse = pick(morph.parse(token), grammeme)
        if parse is None:
            skipped += 1
            continue
        lemma = titlecase(parse.normal_form)
        numbers = ["sing", "plur"] if do_plur else ["sing"]
        for number in numbers:
            for case in CASES:
                inflected = parse.inflect({case, number})
                if inflected is None:
                    continue
                key = (titlecase(inflected.word), lemma)
                if key not in rows:
                    rows[key] = str(inflected.tag)

    freq = f"{args.freq:g}"
    lines = [f"{form}\t{lemma}\t{freq}\t{tags}"
             for (form, lemma), tags in sorted(rows.items())]
    header = (
        "# Russian proper-name surface forms (given names, surnames,\n"
        "# patronymics) with declension, for the long tail of names.\n"
        "# Generated by scripts/names.py from the Natasha name dictionaries\n"
        "# (mawo-nlp-data: first/last/middle, MIT, (c) Alexander Kukushkin)\n"
        "# inflected via mawo-pymorphy3 (OpenCorpora 2025 dicts, CC BY-SA 3.0).\n"
        "# form<TAB>lemma<TAB>freq<TAB>tags  (flat low freq: real words win ties)\n"
    )
    out_dir = os.path.dirname(args.output) or "."
    try:
        fd, tmp = tempfile.mkstemp(dir=out_dir, suffix=".tmp")
    except OSError as e:
        print(f"cannot create temp file in {out_dir}: {e}", file=sys.stderr)
        return 1
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(header)
            f.write("\n".join(lines))
            f.write("\n")
        os.replace(tmp, args.output)
    except BaseException:
        os.unlink(tmp)
        raise
    print(
        f"wrote {args.output}: {len(lines)} name forms "
        f"({len(names)} names, {skipped} unrecognised skipped)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
