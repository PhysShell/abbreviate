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

    override fun noteWord(word: String) {
        engine.noteWord(word)
    }

    override fun resetSession() {
        engine.resetSession()
    }

    override fun setMasking(enabled: Boolean, whenPolite: Boolean) {
        engine.setMasking(enabled, whenPolite)
    }

    companion object {
        /**
         * Engine over the tiny built-in demo lexicon — a fallback that exercises
         * the loop without any asset. The real app loads [fromData].
         */
        fun demo(): UniffiSuggestionPort = UniffiSuggestionPort(AbbrevEngine.withDemoLexicon())

        /**
         * Engine over the real bundled data: the lexicon TSV plus the optional
         * bigram language model (context ranking), conventional shortcuts, and
         * the profanity-mask / tone-marker lists. Takes plain strings, so the
         * port stays Android-free — the host reads the assets. Masking stays
         * **off** until [setMasking] turns it on (a user setting); loading the
         * lists here just makes them available. Throws if [lexiconTsv] is
         * malformed.
         */
        fun fromData(
            lexiconTsv: String,
            lmTsv: String? = null,
            shortcutsTsv: String? = null,
            maskList: String? = null,
            toneMarkers: String? = null,
        ): UniffiSuggestionPort {
            val engine = AbbrevEngine.fromLexiconTsv(lexiconTsv)
            lmTsv?.let { engine.loadLanguageModel(it) }
            shortcutsTsv?.let { engine.loadShortcuts(it) }
            maskList?.let { engine.loadMaskList(it) }
            toneMarkers?.let { engine.loadToneMarkers(it) }
            return UniffiSuggestionPort(engine)
        }
    }
}
