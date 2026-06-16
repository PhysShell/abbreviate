package com.physshell.abbreviate.engine

/** One candidate the strip can offer: the best form for a lemma plus its
 *  sibling forms (for a future "hold for forms" UI). */
data class Candidate(
    val form: String,
    val lemma: String,
    val score: Double,
    val variants: List<String>,
)

/**
 * The engine seam. [com.physshell.abbreviate.controller.SuggestionController]
 * depends on this interface — never on the UniFFI-generated binding directly —
 * so the controller's logic is unit-testable on a plain JVM with a fake, no
 * native `.so` required. [com.physshell.abbreviate.engine.UniffiSuggestionPort]
 * is the production implementation.
 */
interface SuggestionPort {
    fun suggest(input: String, previousWords: List<String>, limit: Int): List<Candidate>

    /** Confirmed-pick learning signal; no-op for fakes that don't learn. */
    fun accept(input: String, form: String) {}
}
