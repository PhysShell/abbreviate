package com.physshell.abbreviate.controller

import com.physshell.abbreviate.engine.SuggestionPort
import com.physshell.abbreviate.host.TextHost

/** One entry rendered in the suggestion strip. */
data class StripItem(val form: String, val lemma: String, val variants: List<String>)

/**
 * Immutable snapshot of the strip the host renders. [token] is the abbreviation
 * currently under the caret; [selected] is the highlighted index (or -1 when
 * empty), driven by keyboard/dpad navigation.
 */
data class StripState(
    val token: String,
    val items: List<StripItem>,
    val selected: Int,
) {
    val isEmpty: Boolean get() = items.isEmpty()
}

/**
 * Host-agnostic glue between the text seam and the engine port. Knows how to
 * carve the token-at-cursor and its left context out of plain text, ask the
 * port for suggestions, track keyboard selection, and commit a pick back
 * through the [TextHost]. No Android, no UniFFI types — so it unit-tests on a
 * plain JVM with a fake port and fake host.
 */
class SuggestionController(
    private val port: SuggestionPort,
    private val limit: Int = 5,
) {
    init {
        // A non-positive limit would wrap to a huge UInt at the FFI boundary.
        require(limit > 0) { "limit must be > 0, was $limit" }
    }

    var state: StripState = EMPTY
        private set

    /** Recompute the strip from the text left of the caret. */
    fun refresh(textBeforeCursor: String): StripState {
        val token = tokenAtCursor(textBeforeCursor)
        if (token.isEmpty()) {
            state = EMPTY
            return state
        }
        val prefix = textBeforeCursor.substring(0, textBeforeCursor.length - token.length)
        val items = port
            .suggest(token, previousWords(prefix), limit)
            .map { StripItem(it.form, it.lemma, it.variants) }
        state = StripState(token, items, if (items.isEmpty()) -1 else 0)
        return state
    }

    /** Move the highlight by [delta], clamped to the strip (no wrap). */
    fun moveSelection(delta: Int): StripState {
        if (state.isEmpty) return state
        val next = (state.selected + delta).coerceIn(0, state.items.size - 1)
        state = state.copy(selected = next)
        return state
    }

    /** Highlight an explicit index (e.g. a digit key or a tap); ignores OOB. */
    fun select(index: Int): StripState {
        if (index in state.items.indices) state = state.copy(selected = index)
        return state
    }

    /**
     * Commit [index] (default: the highlight): replace the token via [host],
     * record the learning signal, and clear the strip. Returns the inserted
     * form, or null if the index is out of range.
     */
    fun accept(host: TextHost, index: Int = state.selected): String? {
        val item = state.items.getOrNull(index) ?: return null
        val token = state.token
        // The strip may be stale: the caret can move (without a text edit)
        // between refresh and accept. Re-read the live text and bail — by
        // recomputing — if the token no longer ends at the caret, so we never
        // replace an unrelated span.
        val before = host.textBeforeCursor()
        if (!before.endsWith(token)) {
            refresh(before)
            return null
        }
        host.replaceTokenAtCursor(token, item.form)
        port.accept(token, item.form)
        state = EMPTY
        return item.form
    }

    /**
     * Feed a committed word into the recency cache. The host decides *when* a
     * pick is committed — explicit picks immediately, a speculative smart-space
     * auto-accept only once its undo window closes — so this passthrough is
     * deliberately separate from [accept]. An empty word is ignored (and the
     * engine filters non-words again).
     */
    fun noteWord(word: String) {
        if (word.isNotEmpty()) port.noteWord(word)
    }

    /**
     * Note the word ending at the caret as a committed word — the host calls
     * this when a separator (space, enter) is about to finish it, so a word the
     * user typed *without* picking a suggestion still warms the recency cache.
     * A non-word tail is ignored here and filtered again by the engine.
     */
    fun noteCommitted(textBeforeCursor: String) {
        noteWord(tokenAtCursor(textBeforeCursor))
    }

    /** Clear the session recency cache (host calls this on a context change). */
    fun resetSession() {
        port.resetSession()
    }

    /**
     * Bind profanity masking to a user setting (§5.2). [whenPolite] tone-gates
     * it — a masked twin is offered only in a polite window (§5.1). Off by
     * default; a passthrough to the port, like [noteWord]/[resetSession].
     */
    fun setMasking(enabled: Boolean, whenPolite: Boolean) {
        port.setMasking(enabled, whenPolite)
    }

    companion object {
        val EMPTY = StripState("", emptyList(), -1)
    }
}

/** Cyrillic letter or hyphen — the alphabet the engine indexes over. */
internal fun isWordChar(c: Char): Boolean =
    c in 'а'..'я' || c in 'А'..'Я' || c == 'ё' || c == 'Ё' || c == '-'

/** The trailing run of word characters ending at the caret (the abbreviation). */
internal fun tokenAtCursor(textBeforeCursor: String): String {
    var i = textBeforeCursor.length
    while (i > 0 && isWordChar(textBeforeCursor[i - 1])) i--
    return textBeforeCursor.substring(i)
}

/** Up to [max] preceding words (left sentence context for the bigram model). */
internal fun previousWords(prefix: String, max: Int = 3): List<String> {
    val words = ArrayList<String>()
    val sb = StringBuilder()
    for (c in prefix) {
        if (isWordChar(c)) {
            sb.append(c)
        } else if (sb.isNotEmpty()) {
            words.add(sb.toString())
            sb.setLength(0)
        }
    }
    if (sb.isNotEmpty()) words.add(sb.toString())
    return if (words.size <= max) words else words.subList(words.size - max, words.size).toList()
}
