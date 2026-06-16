package com.physshell.abbreviate.engine

import uniffi.abbrev_ffi.AbbrevEngine

/**
 * Production [SuggestionPort]: adapts the UniFFI-generated [AbbrevEngine] to the
 * host-agnostic port, mapping grouped suggestions to [Candidate]s. This is the
 * single place the shell touches the generated binding.
 */
class UniffiSuggestionPort(private val engine: AbbrevEngine) : SuggestionPort {

    override fun suggest(input: String, previousWords: List<String>, limit: Int): List<Candidate> =
        engine.suggestGrouped(input, previousWords, limit.toUInt()).map { group ->
            Candidate(
                form = group.best.form,
                lemma = group.lemma,
                score = group.best.score,
                variants = group.variants,
            )
        }

    override fun accept(input: String, form: String) {
        engine.accept(input, form)
    }

    companion object {
        /**
         * Engine over the tiny built-in demo lexicon — enough to exercise the
         * full on-device loop (binding loads, suggestions render, text is
         * replaced) without bundling a lexicon asset. Swap for
         * `AbbrevEngine.fromLexiconTsv(assets.open("ru-50k.tsv")...)` to test
         * against the real lexicon.
         */
        fun demo(): UniffiSuggestionPort = UniffiSuggestionPort(AbbrevEngine.withDemoLexicon())
    }
}
