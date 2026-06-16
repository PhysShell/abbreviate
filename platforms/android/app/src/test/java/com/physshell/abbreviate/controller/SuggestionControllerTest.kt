package com.physshell.abbreviate.controller

import com.physshell.abbreviate.engine.Candidate
import com.physshell.abbreviate.engine.SuggestionPort
import com.physshell.abbreviate.host.TextHost
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Records what the controller asked of the engine and applied to the field. */
private class FakePort(private val canned: List<Candidate>) : SuggestionPort {
    var lastInput: String? = null
    var lastContext: List<String> = emptyList()
    val accepted = mutableListOf<Pair<String, String>>()

    override fun suggest(input: String, previousWords: List<String>, limit: Int): List<Candidate> {
        lastInput = input
        lastContext = previousWords
        return canned
    }

    override fun accept(input: String, form: String) {
        accepted += input to form
    }
}

private class FakeHost : TextHost {
    var before = ""
    val replacements = mutableListOf<Pair<String, String>>()
    override fun textBeforeCursor(): String = before
    override fun replaceTokenAtCursor(token: String, replacement: String) {
        replacements += token to replacement
    }
}

class SuggestionControllerTest {
    private fun cand(form: String) = Candidate(form, form, 1.0, emptyList())

    @Test
    fun extracts_trailing_token_and_left_context() {
        val port = FakePort(listOf(cand("долгосрочная")))
        val c = SuggestionController(port)

        val state = c.refresh("это очень длгсрчная")

        assertEquals("длгсрчная", state.token)
        assertEquals("длгсрчная", port.lastInput)
        assertEquals(listOf("это", "очень"), port.lastContext)
        assertEquals(0, state.selected)
    }

    @Test
    fun caps_context_to_last_three_words() {
        val port = FakePort(listOf(cand("x")))
        SuggestionController(port).refresh("а б в г д првт")
        assertEquals(listOf("в", "г", "д"), port.lastContext)
    }

    @Test
    fun empty_or_non_word_tail_yields_empty_strip() {
        val port = FakePort(listOf(cand("x")))
        val c = SuggestionController(port)
        assertTrue(c.refresh("слово ").isEmpty)
        assertTrue(c.refresh("").isEmpty)
        assertNull(port.lastInput) // engine never consulted for an empty token
    }

    @Test
    fun accept_replaces_token_records_signal_and_clears() {
        val port = FakePort(listOf(cand("привет"), cand("приватный")))
        val host = FakeHost().apply { before = "првт" }
        val c = SuggestionController(port)
        c.refresh(host.before)

        val inserted = c.accept(host, index = 1)

        assertEquals("приватный", inserted)
        assertEquals("првт" to "приватный", host.replacements.single())
        assertEquals("првт" to "приватный", port.accepted.single())
        assertTrue("strip clears after a pick", c.state.isEmpty)
    }

    @Test
    fun selection_navigation_clamps_without_wrapping() {
        val port = FakePort(listOf(cand("a"), cand("b"), cand("c")))
        val c = SuggestionController(port)
        c.refresh("првт")

        assertEquals(0, c.moveSelection(-1).selected) // clamps at the low end
        assertEquals(2, c.moveSelection(5).selected) // clamps at the high end
        assertEquals(1, c.select(1).selected)
        assertEquals(1, c.select(99).selected) // out-of-range ignored
    }

    @Test
    fun accept_rejects_stale_state_when_caret_no_longer_ends_with_token() {
        val port = FakePort(listOf(cand("привет")))
        val host = FakeHost().apply { before = "првт" }
        val c = SuggestionController(port)
        c.refresh(host.before) // token = "првт"

        // Caret moved on (no edit to the token): the live text no longer ends
        // with the token, so accept must replace nothing and recompute.
        host.before = "првт зашёл "
        assertNull(c.accept(host))
        assertTrue("no replacement on stale state", host.replacements.isEmpty())
        assertTrue("strip recomputed (now empty)", c.state.isEmpty)
    }

    @Test
    fun rejects_non_positive_limit() {
        val port = FakePort(emptyList())
        try {
            SuggestionController(port, limit = 0)
            throw AssertionError("expected IllegalArgumentException")
        } catch (_: IllegalArgumentException) {
        }
    }

    @Test
    fun accept_with_no_suggestions_is_a_noop() {
        val host = FakeHost()
        val c = SuggestionController(FakePort(emptyList()))
        c.refresh("првт")
        assertNull(c.accept(host))
        assertTrue(host.replacements.isEmpty())
    }
}
