#!/usr/bin/env python3
"""Corpus miner (offline, build-time): an informal-register Russian corpus in
→ a chat-register frequency table and candidate `abbreviation→word` pairs out.

Reads IlyaGusev/pikabu shards (`NN.jsonl.zst`, columnar `comments`) or any
`.jsonl.zst` / plain `.txt`. Streams and tolerates a truncated zstd tail, so a
byte-range slice of a shard is enough to sample without the full download.

Two outputs:
  * `--freq out.tsv`     — token<TAB>count over post + comment text (the chat
                           register; OpenSubtitles/RNC under-cover it).
  * `--pairs out.tsv`    — real abbreviation candidates: a corpus token that is
                           NOT a lexicon word but whose consonant skeleton
                           matches a frequent lexicon word (зн→знаю, гд→где).
                           These are mined from actual usage, unlike the
                           synthetic generator — the honest eval seed.

The raw corpus is NOT vendored (Pikabu licence); only derived aggregates are
meant to be committed, with attribution. Fetch a shard with, e.g.:
  curl -sSL -H 'Range: bytes=0-60000000' \
    https://huggingface.co/datasets/IlyaGusev/pikabu/resolve/main/00.jsonl.zst \
    -o 00.part.zst

    python3 mine_corpus.py 00.part.zst --lexicon data/lexicons/ru-50k.tsv \
        --freq /tmp/pikabu-freq.tsv --pairs /tmp/pikabu-pairs.tsv
"""

import argparse
import json
import re
import sys

VOWELS = set("аеёиоуыэюя")
SIGNS = set("ьъ")
TAG = re.compile(r"<[^>]+>")
# Cyrillic word tokens (hyphen allowed inside), everything else is a separator.
WORD = re.compile(r"[а-яёА-ЯЁ]+(?:-[а-яёА-ЯЁ]+)*")


def normalize(w):
    """Lowercase and fold `ё→е`, matching the engine's alphabet normalization."""
    return w.lower().replace("ё", "е")


def skeleton(w):
    """Consonant skeleton, matching `abbrev_core::alphabet::skeleton`: drop all
    vowels and the soft/hard signs `ь`/`ъ` (`семья → см`, `знаю → зн`)."""
    return "".join(c for c in w if c not in VOWELS and c not in SIGNS)


def iter_texts(path, max_bytes):
    """Yield raw text blobs (post bodies + comments) from a shard or txt."""
    if path.endswith(".txt"):
        with open(path, encoding="utf-8", errors="ignore") as f:
            for line in f:
                yield line
        return
    import zstandard as zstd

    dctx = zstd.ZstdDecompressor()
    with open(path, "rb") as f:
        reader = dctx.stream_reader(f)
        buf = b""
        try:
            buf = reader.read(max_bytes)
        except zstd.ZstdError:
            pass  # truncated tail from a ranged slice — keep what decoded
    for line in buf.decode("utf-8", "ignore").split("\n"):
        line = line.strip()
        if not line:
            continue
        try:
            o = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(o.get("text_markdown"), str):
            yield o["text_markdown"]
        c = o.get("comments")
        if isinstance(c, dict):
            htmls = c.get("text_html") or c.get("text_markdown") or []
            if isinstance(htmls, list):
                for h in htmls:
                    if isinstance(h, str):
                        yield TAG.sub(" ", h)


def make_tokenizer():
    """Return a tokenizer callable: razdel if installed, else a Cyrillic-word
    regex fallback. razdel splits clitics/punctuation the regex would miss."""
    try:
        from razdel import tokenize as razdel_tok
    except ModuleNotFoundError:
        return lambda s: WORD.findall(s)
    return lambda s: [t.text for t in razdel_tok(s)]


def main():
    """Parse args, count token frequencies, and write the requested TSVs."""
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("--lexicon")
    ap.add_argument("--freq")
    ap.add_argument("--pairs")
    ap.add_argument("--max-bytes", type=int, default=400_000_000)
    ap.add_argument("--min-count", type=int, default=2)
    args = ap.parse_args()
    if args.pairs and not args.lexicon:
        ap.error("--pairs requires --lexicon (skeleton match needs the lexicon)")

    toks = make_tokenizer()
    freq = {}
    n_tok = 0
    for blob in iter_texts(args.corpus, args.max_bytes):
        for raw in toks(blob):
            for m in WORD.findall(raw):
                w = normalize(m)
                freq[w] = freq.get(w, 0) + 1
                n_tok += 1
    print(f"tokens={n_tok} types={len(freq)}", file=sys.stderr)

    if args.freq:
        rows = sorted(freq.items(), key=lambda kv: (-kv[1], kv[0]))
        with open(args.freq, "w", encoding="utf-8") as f:
            f.write("# Pikabu chat-register frequency: token<TAB>count\n")
            f.write("# Derived from IlyaGusev/pikabu (raw corpus not vendored)\n")
            for w, c in rows:
                if c >= args.min_count:
                    f.write(f"{w}\t{c}\n")

    if args.pairs:  # --lexicon is guaranteed present (validated above)
        # skeleton -> (best lexicon word, its freq). Most frequent word wins.
        lex = {}
        skel = {}
        with open(args.lexicon, encoding="utf-8") as f:
            for line in f:
                if line.startswith("#") or not line.strip():
                    continue
                p = line.split("\t")
                if len(p) < 3:
                    continue
                form = normalize(p[0])
                try:
                    fr = float(p[2])
                except ValueError:
                    continue
                lex[form] = fr
                s = skeleton(form)
                if s and (s not in skel or fr > skel[s][1]):
                    skel[s] = (form, fr)
        pairs = []
        for tok, c in freq.items():
            if c < args.min_count or len(tok) < 2 or tok in lex:
                continue  # real lexicon words aren't abbreviations
            cand = skel.get(skeleton(tok))
            if not cand:
                continue
            word, _ = cand
            # An abbreviation is shorter than its expansion and not equal to it.
            if len(tok) < len(word) and tok != word:
                pairs.append((c, tok, word))
        pairs.sort(reverse=True)
        with open(args.pairs, "w", encoding="utf-8") as f:
            f.write("# Mined abbreviation candidates: abbr<TAB>expansion<TAB>corpus_count\n")
            f.write("# Source: IlyaGusev/pikabu (informal register), skeleton match vs lexicon\n")
            for c, tok, word in pairs:
                f.write(f"{tok}\t{word}\t{c}\n")
        print(f"pairs={len(pairs)}", file=sys.stderr)


if __name__ == "__main__":
    sys.exit(main())
