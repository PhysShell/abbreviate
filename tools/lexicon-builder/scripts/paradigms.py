#!/usr/bin/env python3
"""Lexicon pipeline step: generate declension paradigms ("hold groups") from
the lemmas of an engine lexicon.

The hold-popup shows the forms of a lemma when the user long-presses a
suggestion. Returning every form in a pile is a "morphological salad"; the
useful shape is a case-ordered grid, split into groups. This tool emits that
grid so the runtime never has to inflect on device.

It reads the lemma column (2nd) of a lexicon TSV, re-parses each distinct
lemma, and generates the six cases per group via morphological *generation*
(`inflect`). Two parts of speech are covered, keyed by the group column:

  * NOUN — nouns carry lexical gender, so {case, number} fully determines the
    form. One group per number: `sing`, `plur`. Singularia/pluralia tantum
    naturally yield only the number that exists.

  * ADJF — full adjectives and adjectival pronouns (e.g. "мой", "этот",
    "который"; pymorphy tags them ADJF) inflect by gender too, but only in
    the singular. Groups: `sing.masc`, `sing.femn`, `sing.neut`, `plur`. The
    accusative splits by animacy for the masculine singular and the plural
    ("красный"/"красного", "красные"/"красных"); with no head noun to fix
    animacy we emit the inanimate form (which equals the nominative), the
    least-surprising default for a standalone grid.

A lemma that parses as both (a substantivized adjective like "красный") is
emitted as a NOUN — noun coverage takes precedence, so existing noun rows
stay byte-identical when adjectives are added.

Output (sorted, one row per lemma+group, pipe-joined in fixed case order
nomn|gent|datv|accs|ablt|loct; empty slot = no such form):

    lemma<TAB>group<TAB>nomn|gent|datv|accs|ablt|loct
    работа	sing	работа|работы|работе|работу|работой|работе
    работа	plur	работы|работ|работам|работы|работами|работах
    красный	sing.masc	красный|красного|красному|красный|красным|красном
    красный	sing.femn	красная|красной|красной|красную|красной|красной
    красный	sing.neut	красное|красного|красному|красное|красным|красном
    красный	plur	красные|красных|красным|красные|красными|красных

Offline tooling only — never runs on device. Idempotent: same input ->
byte-identical output.

    pip install mawo-pymorphy3        # bundled OpenCorpora 2025 DAWG dicts
    python3 paradigms.py data/lexicons/ru-50k.tsv data/lexicons/ru-hold-groups.tsv
"""

import os
import sys
import tempfile

# Fixed grammatical-case order. Matches abbrev_core::morph case naming so the
# columns line up with the engine's morphology layer.
CASES = ["nomn", "gent", "datv", "accs", "ablt", "loct"]
NUMBERS = ["sing", "plur"]
# Gender axis for adjectives — singular only (the plural has no gender).
GENDERS = ["masc", "femn", "neut"]


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: paradigms.py <lexicon.tsv> <out-hold-groups.tsv>", file=sys.stderr)
        return 1
    src, out = sys.argv[1], sys.argv[2]
    # Validate the input before the heavy import: a bad path should be
    # reported as such, not as a missing dependency.
    try:
        probe = open(src, encoding="utf-8")
    except OSError as e:
        print(f"cannot read {src}: {e}", file=sys.stderr)
        return 1
    with probe:
        lemmas = set()
        for line_no, raw in enumerate(probe, 1):
            line = raw.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            # Enforce the engine's lexicon contract (see lexicon.rs): exactly
            # form<TAB>lemma<TAB>freq, with an optional 4th grammeme column.
            # Anything else is a drifted/broken artifact — fail fast like the
            # engine does, rather than silently building paradigms from it.
            if len(parts) not in (3, 4) or not parts[1]:
                print(
                    f"{src}:{line_no}: expected form<TAB>lemma<TAB>freq[<TAB>tags]",
                    file=sys.stderr,
                )
                return 1
            lemmas.add(parts[1])

    # Lazy import: usage and argument errors must not require the dependency.
    try:
        import mawo_pymorphy3

        morph = mawo_pymorphy3.create_analyzer()
    except ModuleNotFoundError:
        print(
            "mawo-pymorphy3 is not installed; run: pip install mawo-pymorphy3",
            file=sys.stderr,
        )
        return 1

    def cell(parse, grammemes, force_inan_accs=False):
        """One declension cell. Adjectives split the accusative by the head
        noun's animacy (masc singular and plural); with no head noun we force
        the *inanimate* reading — which equals the nominative — for a
        deterministic standalone grid. Nouns carry animacy lexically, so they
        never force it (animacy-ambiguous nouns like "робот" thus keep the
        accusative pymorphy already chose, leaving noun output unchanged)."""
        if force_inan_accs and "accs" in grammemes:
            forced = parse.inflect(grammemes | {"inan"})
            if forced:
                return forced.word
        inflected = parse.inflect(grammemes)
        return inflected.word if inflected else ""

    def pick(parses, lemma, pos):
        # Pick the parse whose normal form is the lemma itself, so we inflect
        # the intended lexeme (e.g. for "стали" the verb "стать" is not what a
        # lemma-keyed paradigm wants).
        return next(
            (p for p in parses if p.tag.POS == pos and p.normal_form == lemma),
            None,
        )

    rows = []
    skipped = 0
    for lemma in lemmas:
        # Parse once; NOUN and ADJF selection reuse the same analyses.
        parses = morph.parse(lemma)
        # Noun coverage takes precedence over adjective coverage, so a
        # substantivized adjective ("красный") keeps its noun paradigm and
        # existing rows stay byte-identical.
        noun = pick(parses, lemma, "NOUN")
        if noun is not None:
            for number in NUMBERS:
                forms = [cell(noun, {case, number}) for case in CASES]
                # Drop a number the lexeme does not have (singularia/pluralia
                # tantum) instead of emitting an all-empty row.
                if any(forms):
                    rows.append(f"{lemma}\t{number}\t" + "|".join(forms))
            continue

        adj = pick(parses, lemma, "ADJF")
        if adj is not None:
            for gender in GENDERS:
                forms = [cell(adj, {case, gender, "sing"}, True) for case in CASES]
                if any(forms):
                    rows.append(f"{lemma}\tsing.{gender}\t" + "|".join(forms))
            plur = [cell(adj, {case, "plur"}, True) for case in CASES]
            if any(plur):
                rows.append(f"{lemma}\tplur\t" + "|".join(plur))
            continue

        skipped += 1

    rows.sort()
    header = (
        "# Russian declension paradigms (hold-popup groups): nouns by number,\n"
        "# adjectives/adjectival pronouns by gender (singular) and number.\n"
        "# Generated by scripts/paradigms.py from the lexicon's lemmas via\n"
        "# mawo-pymorphy3 (OpenCorpora 2025 dicts, CC BY-SA 3.0).\n"
        "# lemma<TAB>group<TAB>nomn|gent|datv|accs|ablt|loct  (empty = no form)\n"
        "# group = sing|plur | sing.masc|sing.femn|sing.neut\n"
    )
    # Atomic replace: a crash mid-write must not corrupt the artifact.
    out_dir = os.path.dirname(out) or "."
    try:
        fd, tmp = tempfile.mkstemp(dir=out_dir, suffix=".tmp")
    except OSError as e:
        print(f"cannot create temp file in {out_dir}: {e}", file=sys.stderr)
        return 1
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(header)
            f.write("\n".join(rows))
            f.write("\n")
        os.replace(tmp, out)
    except BaseException:
        os.unlink(tmp)
        raise
    print(
        f"wrote {out}: {len(rows)} paradigm rows "
        f"({len(lemmas)} lemmas, {skipped} non-declinable skipped)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
