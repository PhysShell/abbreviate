package com.physshell.abbreviate

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.text.Editable
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.TextWatcher
import android.text.style.ForegroundColorSpan
import android.text.style.RelativeSizeSpan
import android.text.style.StyleSpan
import android.util.Log
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.EditText
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import com.physshell.abbreviate.controller.StripState
import com.physshell.abbreviate.controller.SuggestionController
import com.physshell.abbreviate.engine.UniffiSuggestionPort
import com.physshell.abbreviate.host.TextHost

/**
 * A minimal on-device scratchpad: type abbreviations into the field, the strip
 * offers expansions, tap (or dpad / digit keys) to insert. Its only job is to
 * prove the full loop on a real device — the UniFFI binding loads, the engine
 * ranks, and the [SuggestionController]/[TextHost] seam works — without
 * committing to a full keyboard. The same controller will drive the eventual
 * IME/accessibility shells; only the [TextHost] below is shell-specific.
 *
 * The look deliberately mirrors the web demo (`platforms/web`): dark panel,
 * accent-coloured top suggestion, numbered chips.
 */
class ScratchpadActivity : Activity(), TextHost {

    private lateinit var editor: EditText
    private lateinit var strip: LinearLayout
    private lateinit var controller: SuggestionController

    // Guards the TextWatcher against the programmatic edit made by an insertion.
    private var applying = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        controller = SuggestionController(UniffiSuggestionPort.demo())

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(BG)
            setPadding(dp(20), dp(24), dp(20), dp(24))
        }

        root.addView(title())
        root.addView(hint())
        root.addView(
            TextView(this).apply {
                text = "Готово — встроенный демо-словарь загружен."
                setTextColor(MUTED)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
                setPadding(0, dp(4), 0, dp(8))
            },
        )

        strip = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            minimumHeight = dp(42)
        }
        root.addView(
            HorizontalScrollView(this).apply {
                isFillViewport = true
                isHorizontalScrollBarEnabled = false
                addView(strip)
            },
            LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { bottomMargin = dp(8) },
        )

        editor = EditText(this).apply {
            hint = "Начните печатать сокращение, например: ну првт"
            setLines(5)
            gravity = Gravity.TOP or Gravity.START
            setTextColor(INK)
            setHintTextColor(MUTED)
            background = panel(BORDER)
            setPadding(dp(14), dp(12), dp(14), dp(12))
            // Accent focus ring, like `#editor:focus` on the web.
            setOnFocusChangeListener { _, focused -> background = panel(if (focused) ACCENT else BORDER) }
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                override fun afterTextChanged(s: Editable?) {
                    if (!applying) render(controller.refresh(textBeforeCursor()))
                }
            })
        }
        root.addView(editor, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))

        setContentView(root)
        editor.requestFocus()
    }

    // --- TextHost: the only shell-specific text plumbing -------------------

    override fun textBeforeCursor(): String {
        val end = editor.selectionStart.coerceIn(0, editor.text.length)
        return editor.text.substring(0, end)
    }

    override fun replaceTokenAtCursor(token: String, replacement: String) {
        val caret = editor.selectionStart.coerceIn(0, editor.text.length)
        val start = (caret - token.length).coerceAtLeast(0)
        // Defensive: only replace if the span really is the token (the
        // controller guards too, but the field can shift under us).
        if (editor.text.substring(start, caret) != token) return
        applying = true
        try {
            // Insert "form " (trailing space) so the next abbreviation starts clean,
            // matching the web demo's accept behaviour.
            editor.text.replace(start, caret, "$replacement ")
            editor.setSelection(start + replacement.length + 1)
        } finally {
            applying = false
        }
    }

    // --- Header ------------------------------------------------------------

    private fun title(): TextView {
        val s = SpannableStringBuilder("abbreviate  демо")
        s.setSpan(StyleSpan(Typeface.BOLD), 0, 10, Spanned.SPAN_INCLUSIVE_EXCLUSIVE)
        s.setSpan(ForegroundColorSpan(MUTED), 10, s.length, Spanned.SPAN_INCLUSIVE_EXCLUSIVE)
        s.setSpan(RelativeSizeSpan(0.6f), 10, s.length, Spanned.SPAN_INCLUSIVE_EXCLUSIVE)
        return TextView(this).apply {
            text = s
            setTextColor(INK)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 24f)
            setPadding(0, 0, 0, dp(4))
        }
    }

    private fun hint(): TextView {
        val full = "Пишите сокращение без гласных — движок предлагает полное слово. " +
            "Примеры: првт, тстрние, сгдня, рбте."
        val s = SpannableStringBuilder(full)
        // Tint the example abbreviations with the accent, like the <code> chips.
        for (ex in listOf("првт", "тстрние", "сгдня", "рбте")) {
            val at = full.indexOf(ex)
            if (at >= 0) s.setSpan(ForegroundColorSpan(ACCENT), at, at + ex.length, Spanned.SPAN_INCLUSIVE_EXCLUSIVE)
        }
        return TextView(this).apply {
            text = s
            setTextColor(MUTED)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            setPadding(0, 0, 0, dp(12))
        }
    }

    // --- Strip rendering + selection --------------------------------------

    private fun render(state: StripState) {
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
                    controller.select(i)
                    accept(i)
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

    /** Commit suggestion [index] and recompute the strip from the new caret. */
    private fun accept(index: Int) {
        val form = controller.accept(this, index)
        Log.d(TAG, "accept[$index] → ${form ?: "(stale)"}")
        render(controller.refresh(textBeforeCursor()))
    }

    // Hardware-keyboard / dpad navigation. On a soft keyboard the strip is
    // tap-driven; this mirrors the digit/arrow selection the web demo and the
    // eventual IME shell use.
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (controller.state.isEmpty) return super.onKeyDown(keyCode, event)
        when (keyCode) {
            KeyEvent.KEYCODE_DPAD_LEFT -> render(controller.moveSelection(-1))
            KeyEvent.KEYCODE_DPAD_RIGHT -> render(controller.moveSelection(1))
            KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER -> accept(controller.state.selected)
            in KeyEvent.KEYCODE_1..KeyEvent.KEYCODE_9 -> {
                val idx = keyCode - KeyEvent.KEYCODE_1
                if (idx >= controller.state.items.size) return super.onKeyDown(keyCode, event)
                accept(idx)
            }
            else -> return super.onKeyDown(keyCode, event)
        }
        return true
    }

    // --- Styling helpers (palette mirrors platforms/web/style.css) ---------

    private fun panel(stroke: Int) = GradientDrawable().apply {
        setColor(PANEL)
        cornerRadius = dp(10).toFloat()
        setStroke(dp(1), stroke)
    }

    private fun chipBackground(selected: Boolean) = GradientDrawable().apply {
        setColor(if (selected) HOVER else PANEL)
        cornerRadius = dp(10).toFloat()
        setStroke(dp(1), if (selected) ACCENT else BORDER)
    }

    private fun badgeBackground(selected: Boolean) = GradientDrawable().apply {
        setColor(Color.TRANSPARENT)
        cornerRadius = dp(4).toFloat()
        setStroke(dp(1), if (selected) ACCENT else BORDER)
    }

    private fun dp(v: Int): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    companion object {
        private const val TAG = "Abbrev"

        // Palette lifted from platforms/web/style.css :root.
        private val BG = Color.parseColor("#16181D")
        private val PANEL = Color.parseColor("#1F2229")
        private val INK = Color.parseColor("#E8EAED")
        private val MUTED = Color.parseColor("#9AA0A6")
        private val ACCENT = Color.parseColor("#6EA8FE")
        private val BORDER = Color.parseColor("#2C303A")
        private val HOVER = Color.parseColor("#272B34")
    }
}
