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
 * Abbreviation keyboard as an [InputMethodService]. This is the IME shell over
 * the same seam as the scratchpad: it reuses [SuggestionController] and the
 * engine port verbatim and implements only [TextHost] — here on top of an
 * [android.view.inputmethod.InputConnection] (clean token replacement via
 * deleteSurroundingText + commitText), instead of the scratchpad's EditText.
 *
 * The keyboard itself is deliberately minimal (ЙЦУКЕН letters + space /
 * backspace / enter) — enough to type abbreviations in any field and prove the
 * loop works behind a real input method. No INTERNET permission: fully offline.
 */
class AbbrevImeService : InputMethodService(), TextHost {

    private val main = Handler(Looper.getMainLooper())
    private lateinit var strip: LinearLayout
    // Set once the engine has loaded on the background thread.
    private var controller: SuggestionController? = null

    @Volatile
    private var destroyed = false

    override fun onCreate() {
        super.onCreate()
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

    override fun onCreateInputView(): View {
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

        for (lettersRow in ROWS) root.addView(letterRow(lettersRow))
        root.addView(bottomRow())
        return root
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
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

    // --- typing + suggestions ---------------------------------------------

    private fun type(text: String) {
        currentInputConnection?.commitText(text, 1)
        refresh()
    }

    private fun backspace() {
        currentInputConnection?.deleteSurroundingText(1, 0)
        refresh()
    }

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
                        c.select(i)
                        c.accept(this@AbbrevImeService, i)
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

    private fun letterRow(letters: String): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)
            for (ch in letters) addView(key(ch.toString()) { type(ch.toString()) })
        }

    private fun bottomRow(): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)
            addView(key("⌫", 1.6f) { backspace() })
            addView(key("пробел", 5f) { type(" ") })
            addView(key("↵", 1.6f) { sendDefaultEditorAction(true) })
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
        private val ROWS = listOf("йцукенгшщзхъ", "фывапролджэ", "ячсмитьбю")

        private val BG = Color.parseColor("#16181D")
        private val PANEL = Color.parseColor("#1F2229")
        private val INK = Color.parseColor("#E8EAED")
        private val MUTED = Color.parseColor("#9AA0A6")
        private val ACCENT = Color.parseColor("#6EA8FE")
        private val BORDER = Color.parseColor("#2C303A")
        private val HOVER = Color.parseColor("#272B34")
    }
}
