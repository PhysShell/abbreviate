package com.physshell.abbreviate.host

/**
 * The thin, host-specific text seam — the *only* part that gets rewritten when
 * moving between delivery shells (scratchpad EditText now; an
 * AccessibilityService overlay or an IME's InputConnection later). Everything
 * above it ([com.physshell.abbreviate.controller.SuggestionController] and the
 * engine port) is host-agnostic and carries over unchanged.
 *
 * Two operations are all the controller needs:
 *  - read the text immediately left of the cursor (to find the token to expand
 *    and its sentence context);
 *  - replace that trailing token with the chosen expansion.
 */
interface TextHost {
    /** Text from the start of the field up to the caret (caret-exclusive). */
    fun textBeforeCursor(): String

    /**
     * Replace the [token] that ends at the caret with [replacement], leaving the
     * caret just after the inserted text. [token] is passed back (rather than
     * re-derived) so the host replaces exactly what the controller matched.
     */
    fun replaceTokenAtCursor(token: String, replacement: String)
}
