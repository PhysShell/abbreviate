//! # abbrev-core
//!
//! Offline engine for a Russian abbreviation IME: recovers full words from
//! consonant-heavy / vowel-omitted shorthand (`првт` → `привет`,
//! `тстрние` → `тестирование`) and ranks candidates.
//!
//! Architecture (see `docs/ARCHITECTURE.md` at the repo root):
//!
//! 1. **Lexicon layer** — surface forms with lemma and frequency
//!    ([`Lexicon`]).
//! 2. **Index layer** — consonant-skeleton index, plain prefix index and a
//!    reverse-suffix index ([`index::Indexes`]).
//! 3. **Candidate generation** — capped union of index buckets filtered by
//!    a weighted edit distance with cheap vowel insertions ([`edit`]).
//! 4. **Ranking** — linear model over skeleton/suffix/edit/frequency/
//!    context/user signals ([`rank`]).
//! 5. **Personalization** — local history of accepted suggestions
//!    ([`history::UserHistory`]).
//!
//! Design invariants:
//!
//! * **Sans-IO**: the crate never touches files, network or threads.
//! * **Zero dependencies**: portable to Android/iOS/WASM/desktop as-is.
//! * **Deterministic**: same lexicon + history + input ⇒ same output.

pub mod alphabet;
pub mod context;
pub mod edit;
pub mod engine;
pub mod history;
pub mod index;
pub mod lexicon;
pub mod ngram;
pub mod rank;
pub mod shortcuts;

pub use context::{Context, ContextModel, NoContext};
pub use edit::EditCosts;
pub use engine::{Engine, EngineConfig, Suggestion, SuggestionGroup};
pub use lexicon::{Lexicon, LexiconEntry, LexiconError};
pub use ngram::{BigramModel, LmError};
pub use rank::Weights;
pub use shortcuts::{ShortcutError, Shortcuts};
