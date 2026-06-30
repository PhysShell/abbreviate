package com.physshell.abbreviate.ime

import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.inputmethod.EditorInfo
import android.widget.Button
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import com.physshell.abbreviate.EngineLoader
import com.physshell.abbreviate.controller.StripState
import com.physshell.abbreviate.controller.SuggestionController
import com.physshell.abbreviate.host.TextHost
import kotlin.concurrent.thread

/**
 * Abbreviation keyboard as an [InputMethodService] — the IME shell over the same
 * seam as the scratchpad: it reuses [SuggestionController] and the engine port
 * verbatim and implements only [TextHost] (over [android.view.inputmethod.InputConnection]).
 *
 * UX beyond the bare loop:
 *  - backspace lives at the right end of the bottom letter row (away from the
 *    system "hide keyboard" chevron in the bottom-left corner, which is easy to
 *    hit by accident); the bottom-left key is the harmless layout toggle;
 *  - space accepts the top suggestion when the strip is non-empty (and an
 *    immediate backspace reverts it back to the typed abbreviation);
 *  - an EN/РУ toggle flips to a Latin QWERTY (type Latin without changing the
 *    system layout); a "тр" key transliterates the current selection
 *    Cyrillic→Latin (dumb, letter-by-letter);
 *  - a "⇄" key flips the Cyrillic side between the familiar full ЙЦУКЕН (kept
 *    for people used to it — the default) and a consonant-first layout that
 *    matches what this IME is actually for: the main rows hold only consonants
 *    (in their ЙЦУКЕН positions, vowels removed), with a thin vowel row on top
 *    that collapses to a slim handle via a "▴/▾" toggle. The layout choice and
 *    the vowel-row visibility are remembered across sessions (SharedPreferences).
 *
 * No INTERNET permission: fully offline.
 */
class AbbrevImeService : InputMethodService(), TextHost {

    private val main = Handler(Looper.getMainLooper())
    private lateinit var strip: LinearLayout
    // Set once the engine has loaded on the background thread.
    private var controller: SuggestionController? = null

    @Volatile
    private var destroyed = false

    // Latin QWERTY instead of ЙЦУКЕН when true (the EN/РУ toggle).
    private var latin = false

    // Cyrillic side: consonant-first layout when true, full ЙЦУКЕН (default) when
    // false. When consonant, [vowelsVisible] toggles the thin top vowel row.
    // Both persist across sessions (see [prefs]).
    private var ruConsonant = false
    private var vowelsVisible = true

    private val prefs by lazy { getSharedPreferences("abbrev_ime", MODE_PRIVATE) }

    // Last space-triggered auto-accept: (typed token, inserted form). If the very
    // next key is backspace we restore the token instead of deleting a char.
    private var lastAutoAccept: Pair<String, String>? = null

    // Package of the field we are currently attached to. The session recency
    // cache is scoped per app: when this changes we reset it (see onStartInputView).
    private var currentPackage: String? = null

    override fun onCreate() {
        super.onCreate()
        ruConsonant = prefs.getBoolean(KEY_CONSONANT, false)
        vowelsVisible = prefs.getBoolean(KEY_VOWELS, true)
        // ~11 MB of TSV: parse + build the index off the main thread.
        thread(name = "abbrev-ime-load") {
            val loaded = EngineLoader.fromAssets(assets)
            main.post {
                if (destroyed) return@post // load outlived the service
                controller = SuggestionController(loaded.port)
                refresh()
            }
        }
    }

    override fun onDestroy() {
        destroyed = true
        super.onDestroy()
    }

    override fun onCreateInputView(): View = buildKeyboard()

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        keepAutoAccept() // leaving the old field keeps any pending auto-accept
        // Per-app session cache: switching to a different app's field clears the
        // ephemeral recency cache, so a word learned in one app doesn't leak
        // into another. The IME only sees the package, not *which chat* inside
        // an app, so this is per-app, not per-conversation (see docs/RESEARCH-
        // RECENCY-CACHE.md §5).
        val pkg = info?.packageName
        if (pkg != currentPackage) {
            controller?.resetSession()
            currentPackage = pkg
        }
        refresh() // recompute against whatever field we just attached to
    }

    // --- TextHost over InputConnection ------------------------------------

    override fun textBeforeCursor(): String =
        currentInputConnection?.getTextBeforeCursor(64, 0)?.toString().orEmpty()

    override fun replaceTokenAtCursor(token: String, replacement: String) {
        val ic = currentInputConnection ?: return
        ic.beginBatchEdit()
        ic.deleteSurroundingText(token.length, 0)
        ic.commitText("$replacement ", 1) // trailing space, like the scratchpad
        ic.endBatchEdit()
    }

    // --- key actions -------------------------------------------------------

    /**
     * A smart-space auto-accept was kept — any action other than the immediate
     * revert-[onBackspace] supersedes it — so its inserted form is now a
     * committed word: note it and disarm the undo. No-op when nothing pending.
     * Deferring the recency note to here (instead of at accept time) means a
     * suggestion the user instantly reverts never warms the cache.
     */
    private fun keepAutoAccept() {
        val pending = lastAutoAccept ?: return
        lastAutoAccept = null
        controller?.noteWord(pending.second) // the inserted form
    }

    /** A plain character key (letter): commit it and recompute. */
    private fun onKey(text: String) {
        keepAutoAccept()
        currentInputConnection?.commitText(text, 1)
        refresh()
    }

    /**
     * Smart space: if the strip is non-empty, accept the top suggestion
     * (instead of inserting a literal space) and arm a one-step undo; otherwise
     * insert a normal space.
     */
    private fun onSpace() {
        keepAutoAccept() // a previous auto-accept survived; settle it first
        val c = controller
        if (c != null && !c.state.isEmpty) {
            val token = c.state.token
            // Speculative: don't note the form yet — it's deferred until the
            // undo window closes (keepAutoAccept), so an instant revert won't
            // warm the cache.
            val form = c.accept(this, 0) // always the top suggestion
            if (form != null) {
                lastAutoAccept = token to form
                refresh()
                return
            }
        }
        // No suggestion taken: the word the user just typed is committed by this
        // space — feed it to the recency cache before inserting the separator.
        c?.noteCommitted(textBeforeCursor())
        onKey(" ")
    }

    /**
     * Backspace. If the previous key was a smart-space auto-accept, revert it
     * (restore the abbreviation) instead of deleting a character. Otherwise a
     * normal single-char delete.
     */
    private fun onBackspace() {
        val ic = currentInputConnection
        val pending = lastAutoAccept
        lastAutoAccept = null
        if (ic != null && pending != null) {
            val (token, form) = pending
            val inserted = "$form "
            if (ic.getTextBeforeCursor(inserted.length, 0)?.toString() == inserted) {
                // Immediate revert: the user rejected the auto-accept, so it is
                // *not* a committed word — deliberately don't note it.
                ic.beginBatchEdit()
                ic.deleteSurroundingText(inserted.length, 0)
                ic.commitText(token, 1)
                ic.endBatchEdit()
                refresh()
                return
            }
            // Not the revert (the inserted text already changed): the auto-accept
            // was kept, so note it before the normal delete.
            controller?.noteWord(form)
        }
        ic?.deleteSurroundingText(1, 0)
        refresh()
    }

    private fun onEnter() {
        keepAutoAccept()
        // The word before the caret is committed by enter — note it too.
        controller?.noteCommitted(textBeforeCursor())
        sendDefaultEditorAction(true)
    }

    private fun toggleLayout() {
        latin = !latin
        keepAutoAccept()
        setInputView(buildKeyboard())
        refresh()
    }

    /** Flip the Cyrillic side between full ЙЦУКЕН and the consonant-first layout. */
    private fun toggleRuLayout() {
        ruConsonant = !ruConsonant
        prefs.edit().putBoolean(KEY_CONSONANT, ruConsonant).apply()
        keepAutoAccept()
        setInputView(buildKeyboard())
        refresh() // setInputView rebuilds the strip view; repopulate it
    }

    /** Show/hide the thin vowel row (consonant layout only). */
    private fun toggleVowels() {
        vowelsVisible = !vowelsVisible
        prefs.edit().putBoolean(KEY_VOWELS, vowelsVisible).apply()
        setInputView(buildKeyboard())
        refresh() // setInputView rebuilds the strip view; repopulate it
    }

    /** Transliterate the current selection Cyrillic→Latin, in place. */
    private fun onTranslit() {
        val ic = currentInputConnection ?: return
        val selected = ic.getSelectedText(0)?.toString()
        if (selected.isNullOrEmpty()) return
        ic.commitText(translit(selected), 1) // replaces the selection
        keepAutoAccept()
        refresh()
    }

    // --- suggestions -------------------------------------------------------

    private fun refresh() {
        val c = controller ?: return
        render(c.refresh(textBeforeCursor()))
    }

    private fun render(state: StripState) {
        if (!::strip.isInitialized) return
        strip.removeAllViews()
        state.items.forEachIndexed { i, item ->
            val selected = i == state.selected
            val chip = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                background = chipBackground(selected)
                setPadding(dp(8), dp(6), dp(10), dp(6))
                isClickable = true
                setOnClickListener {
                    controller?.let { c ->
                        keepAutoAccept()
                        c.select(i)
                        // Explicit pick (no undo window): note it right away.
                        val picked = c.accept(this@AbbrevImeService, i)
                        if (picked != null) c.noteWord(picked)
                        refresh()
                    }
                }
            }
            if (i < 9) {
                chip.addView(
                    TextView(this).apply {
                        text = (i + 1).toString()
                        gravity = Gravity.CENTER
                        setTextColor(if (selected) ACCENT else MUTED)
                        setTextSize(TypedValue.COMPLEX_UNIT_SP, 11f)
                        background = badgeBackground(selected)
                        minWidth = dp(18)
                        setPadding(dp(5), dp(1), dp(5), dp(1))
                    },
                    LinearLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT).apply { rightMargin = dp(6) },
                )
            }
            chip.addView(
                TextView(this).apply {
                    text = item.form
                    setTextColor(if (selected) ACCENT else INK)
                    setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
                    setTypeface(typeface, if (selected) Typeface.BOLD else Typeface.NORMAL)
                },
            )
            strip.addView(
                chip,
                LinearLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT).apply { rightMargin = dp(8) },
            )
        }
    }

    // --- keyboard ----------------------------------------------------------

    private fun buildKeyboard(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(BG)
            setPadding(dp(4), dp(4), dp(4), dp(6))
        }

        strip = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            minimumHeight = dp(46)
        }
        root.addView(
            HorizontalScrollView(this).apply {
                isFillViewport = true
                isHorizontalScrollBarEnabled = false
                addView(strip)
            },
            LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { bottomMargin = dp(4) },
        )

        // Consonant layout: a thin vowel row on top, collapsible to a slim handle.
        if (!latin && ruConsonant) {
            root.addView(if (vowelsVisible) vowelRow() else vowelHandle())
        }

        val rows = when {
            latin -> LATIN_ROWS
            ruConsonant -> CONSONANT_ROWS
            else -> RU_ROWS
        }
        rows.forEachIndexed { i, letters ->
            val row = letterRow()
            for (ch in letters) row.addView(key(ch.toString()) { onKey(ch.toString()) })
            // Backspace on the right end of the last letter row (not bottom-left,
            // where the system "hide keyboard" button sits).
            if (i == rows.lastIndex) row.addView(key("⌫", 1.6f) { onBackspace() })
            root.addView(row)
        }
        root.addView(bottomRow())
        return root
    }

    private fun letterRow(): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)
        }

    /** Thin row of vowels above the consonant rows; "▴" collapses it. */
    private fun vowelRow(): LinearLayout =
        letterRow().apply {
            for (ch in VOWELS) addView(vowelKey(ch.toString()) { onKey(ch.toString()) })
            addView(vowelKey("▴") { toggleVowels() })
        }

    /** Slim full-width handle that re-expands the hidden vowel row. */
    private fun vowelHandle(): LinearLayout =
        letterRow().apply { addView(vowelKey("гласные ▾") { toggleVowels() }) }

    private fun bottomRow(): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)
            addView(key(if (latin) "РУ" else "EN", 1.6f) { toggleLayout() })
            // Consonant/full ЙЦУКЕН switch — only meaningful on the Cyrillic side.
            if (!latin) addView(key("⇄", 1.4f) { toggleRuLayout() })
            addView(key("тр", 1.4f) { onTranslit() })
            addView(key("пробел", 4.5f) { onSpace() })
            addView(key("↵", 1.6f) { onEnter() })
        }

    private fun key(label: String, weight: Float = 1f, onClick: () -> Unit): Button =
        Button(this).apply {
            text = label
            isAllCaps = false
            setTextColor(INK)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 17f)
            background = keyBackground()
            minWidth = 0
            minimumWidth = 0
            setPadding(0, dp(11), 0, dp(11))
            setOnClickListener { onClick() }
            layoutParams = LinearLayout.LayoutParams(0, WRAP_CONTENT, weight)
                .apply { setMargins(dp(2), dp(2), dp(2), dp(2)) }
        }

    /** A shorter, smaller key for the thin vowel row (and its toggle/handle). */
    private fun vowelKey(label: String, weight: Float = 1f, onClick: () -> Unit): Button =
        Button(this).apply {
            text = label
            isAllCaps = false
            setTextColor(INK)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            background = keyBackground()
            minWidth = 0
            minimumWidth = 0
            setPadding(0, dp(6), 0, dp(6))
            setOnClickListener { onClick() }
            layoutParams = LinearLayout.LayoutParams(0, WRAP_CONTENT, weight)
                .apply { setMargins(dp(2), dp(2), dp(2), dp(2)) }
        }

    /** Dumb letter-by-letter Cyrillic→Latin transliteration (case preserved). */
    private fun translit(s: String): String {
        val sb = StringBuilder(s.length)
        for (ch in s) {
            val rep = TRANSLIT[ch.lowercaseChar()]
            when {
                rep == null -> sb.append(ch)
                ch.isUpperCase() && rep.isNotEmpty() -> sb.append(rep.replaceFirstChar { it.uppercaseChar() })
                else -> sb.append(rep)
            }
        }
        return sb.toString()
    }

    // --- styling (palette mirrors platforms/web/style.css) ----------------

    private fun rounded(fill: Int, stroke: Int, radius: Int) = GradientDrawable().apply {
        setColor(fill)
        cornerRadius = dp(radius).toFloat()
        setStroke(dp(1), stroke)
    }

    private fun keyBackground() = rounded(PANEL, BORDER, 8)
    private fun chipBackground(selected: Boolean) = rounded(if (selected) HOVER else PANEL, if (selected) ACCENT else BORDER, 10)
    private fun badgeBackground(selected: Boolean) = rounded(Color.TRANSPARENT, if (selected) ACCENT else BORDER, 4)

    private fun dp(v: Int): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    companion object {
        private const val KEY_CONSONANT = "ru_consonant"
        private const val KEY_VOWELS = "vowels_visible"

        private val RU_ROWS = listOf("йцукенгшщзхъ", "фывапролджэ", "ячсмитьбю")
        // RU_ROWS with the vowels (а е ё и о у ы э ю я) dropped — consonants keep
        // their ЙЦУКЕН positions, so muscle memory mostly carries over.
        private val CONSONANT_ROWS = listOf("йцкнгшщзхъ", "фвпрлдж", "чсмтьб")
        // The dropped vowels, surfaced in the thin top row.
        private const val VOWELS = "аеиоуыяюэё"
        private val LATIN_ROWS = listOf("qwertyuiop", "asdfghjkl", "zxcvbnm")

        // Plain, opinion-free RU→Latin map (the "dumb" transliteration).
        private val TRANSLIT = mapOf(
            'а' to "a", 'б' to "b", 'в' to "v", 'г' to "g", 'д' to "d", 'е' to "e",
            'ё' to "e", 'ж' to "zh", 'з' to "z", 'и' to "i", 'й' to "j", 'к' to "k",
            'л' to "l", 'м' to "m", 'н' to "n", 'о' to "o", 'п' to "p", 'р' to "r",
            'с' to "s", 'т' to "t", 'у' to "u", 'ф' to "f", 'х' to "h", 'ц' to "c",
            'ч' to "ch", 'ш' to "sh", 'щ' to "shch", 'ъ' to "", 'ы' to "y", 'ь' to "",
            'э' to "e", 'ю' to "yu", 'я' to "ya",
        )

        private val BG = Color.parseColor("#16181D")
        private val PANEL = Color.parseColor("#1F2229")
        private val INK = Color.parseColor("#E8EAED")
        private val MUTED = Color.parseColor("#9AA0A6")
        private val ACCENT = Color.parseColor("#6EA8FE")
        private val BORDER = Color.parseColor("#2C303A")
        private val HOVER = Color.parseColor("#272B34")
    }
}
