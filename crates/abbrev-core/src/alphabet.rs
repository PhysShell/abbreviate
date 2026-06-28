//! Russian alphabet utilities: normalization, consonant skeletons and
//! ЙЦУКЕН keyboard geometry.

/// Russian vowels (lowercase, `ё` is folded to `е` by [`normalize`]).
pub const VOWELS: [char; 10] = ['а', 'е', 'ё', 'и', 'о', 'у', 'ы', 'э', 'ю', 'я'];

/// Returns `true` for a Russian vowel (case-insensitive).
pub fn is_vowel(c: char) -> bool {
    let c = lower(c);
    VOWELS.contains(&c)
}

/// Returns `true` for `ь`/`ъ` — signs the user frequently omits.
pub fn is_sign(c: char) -> bool {
    matches!(lower(c), 'ь' | 'ъ')
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Canonical form used everywhere inside the engine: lowercase, `ё` → `е`.
///
/// Display forms keep their original spelling in the lexicon; only the
/// matching keys are normalized.
pub fn normalize(s: &str) -> String {
    s.chars()
        .map(lower)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

/// Consonant skeleton: drops vowels and `ь`/`ъ`, keeps everything else
/// in order. `привет` → `првт`, `тестирование` → `тстрвн`.
///
/// The input is expected to be [`normalize`]d.
pub fn skeleton(s: &str) -> String {
    s.chars().filter(|&c| !is_vowel(c) && !is_sign(c)).collect()
}

/// Whether `s` is a plain Russian word — the engine's "если не уверен — не
/// трогай" predicate. Russian letters with at most *internal* single hyphens
/// (`кто-то`); digits, Latin, punctuation or symbols (`пароль1`, `привет!`, a
/// URL) make it a non-word the engine must not reason about, neither as input
/// nor as a learned session word. A leading/trailing/doubled hyphen or a
/// hyphen-only string (`-`, `--`, `слово-`) is rejected too. An empty string
/// is vacuously plain (callers guard length separately).
pub fn is_plain_russian(s: &str) -> bool {
    let mut prev_hyphen = false;
    let mut seen_letter = false;
    for c in s.chars() {
        match c {
            'а'..='я' | 'ё' => {
                seen_letter = true;
                prev_hyphen = false;
            }
            // A hyphen is only valid between letters: not leading, not doubled.
            '-' if seen_letter && !prev_hyphen => prev_hyphen = true,
            _ => return false,
        }
    }
    !prev_hyphen // reject a trailing hyphen
}

/// ЙЦУКЕН layout rows used for adjacency checks.
const ROWS: [&str; 3] = ["йцукенгшщзхъ", "фывапролджэ", "ячсмитьбю"];

fn key_position(c: char) -> Option<(usize, usize)> {
    for (row, letters) in ROWS.iter().enumerate() {
        if let Some(col) = letters.chars().position(|l| l == c) {
            return Some((row, col));
        }
    }
    None
}

/// Returns `true` if two letters sit on neighboring ЙЦУКЕН keys.
/// Rows are treated as vertically aligned, which is a close enough
/// approximation of the physical stagger for cost modelling.
pub fn keyboard_adjacent(a: char, b: char) -> bool {
    let (a, b) = (lower(a), lower(b));
    if a == b {
        return false;
    }
    match (key_position(a), key_position(b)) {
        (Some((ra, ca)), Some((rb, cb))) => {
            let dr = ra.abs_diff(rb);
            let dc = ca.abs_diff(cb);
            (dr == 0 && dc == 1) || (dr == 1 && dc <= 1)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_yo() {
        assert_eq!(normalize("Ёжик"), "ежик");
        assert_eq!(normalize("ПРИВЕТ"), "привет");
    }

    #[test]
    fn skeleton_drops_vowels_and_signs() {
        assert_eq!(skeleton("привет"), "првт");
        assert_eq!(skeleton("тестирование"), "тстрвн");
        assert_eq!(skeleton("тстрние"), "тстрн");
        assert_eq!(skeleton("семья"), "см");
    }

    #[test]
    fn plain_russian_allows_only_internal_hyphens() {
        for w in ["привет", "кто-то", "что-нибудь", "ёж", ""] {
            assert!(is_plain_russian(w), "{w:?} should be plain");
        }
        for w in [
            "-",
            "--",
            "-слово",
            "слово-",
            "кто--то",
            "пароль1",
            "привет!",
            "api",
        ] {
            assert!(!is_plain_russian(w), "{w:?} should be rejected");
        }
    }

    #[test]
    fn keyboard_adjacency() {
        assert!(keyboard_adjacent('а', 'п')); // same row
        assert!(keyboard_adjacent('а', 'е')); // row above
        assert!(!keyboard_adjacent('й', 'э'));
        assert!(!keyboard_adjacent('а', 'а'));
    }
}
