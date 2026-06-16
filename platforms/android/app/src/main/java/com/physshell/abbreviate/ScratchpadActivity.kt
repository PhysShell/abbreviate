package com.physshell.abbreviate

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.text.Editable
import android.text.TextWatcher
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.EditText
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import com.physshell.abbreviate.controller.SuggestionController
import com.physshell.abbreviate.controller.StripState
import com.physshell.abbreviate.engine.UniffiSuggestionPort
import com.physshell.abbreviate.host.TextHost

/**
 * A minimal on-device scratchpad: type abbreviations into the field, the strip
 * offers expansions, tap (or dpad + center) to insert. Its only job is to prove
 * the full loop on a real device — the UniFFI binding loads, the engine ranks,
 * and the [SuggestionController]/[TextHost] seam works — without committing to a
 * full keyboard. The same controller will drive the eventual IME/accessibility
 * shells; only the [TextHost] below is shell-specific.
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
            setPadding(dp(16), dp(16), dp(16), dp(16))
        }

        root.addView(
            TextView(this).apply {
                text = "Печатай сокращение, например «првт» — подсказки снизу."
                setPadding(0, 0, 0, dp(8))
            },
        )

        editor = EditText(this).apply {
            hint = "Текст…"
            setLines(4)
            gravity = Gravity.TOP or Gravity.START
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                override fun afterTextChanged(s: Editable?) {
                    if (!applying) render(controller.refresh(textBeforeCursor()))
                }
            })
        }
        root.addView(editor, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))

        strip = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        root.addView(
            HorizontalScrollView(this).apply {
                isFillViewport = true
                addView(strip)
            },
            LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { topMargin = dp(8) },
        )

        setContentView(root)
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
            editor.text.replace(start, caret, replacement)
            editor.setSelection(start + replacement.length)
        } finally {
            applying = false
        }
    }

    // --- Strip rendering + selection --------------------------------------

    private fun render(state: StripState) {
        strip.removeAllViews()
        state.items.forEachIndexed { i, item ->
            strip.addView(
                Button(this).apply {
                    text = "${i + 1}. ${item.form}"
                    isAllCaps = false
                    typeface = if (i == state.selected) Typeface.DEFAULT_BOLD else Typeface.DEFAULT
                    setBackgroundColor(if (i == state.selected) HIGHLIGHT else Color.TRANSPARENT)
                    setOnClickListener {
                        controller.select(i)
                        controller.accept(this@ScratchpadActivity, i)
                        render(controller.refresh(textBeforeCursor()))
                    }
                },
            )
        }
    }

    // Hardware-keyboard / dpad navigation. On a soft keyboard the strip is
    // tap-driven; this mirrors the digit/arrow selection the IME shell will use.
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (controller.state.isEmpty) return super.onKeyDown(keyCode, event)
        when (keyCode) {
            KeyEvent.KEYCODE_DPAD_LEFT -> render(controller.moveSelection(-1))
            KeyEvent.KEYCODE_DPAD_RIGHT -> render(controller.moveSelection(1))
            KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER -> {
                controller.accept(this)
                render(controller.refresh(textBeforeCursor()))
            }
            else -> return super.onKeyDown(keyCode, event)
        }
        return true
    }

    private fun dp(v: Int): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    companion object {
        private val HIGHLIGHT = Color.parseColor("#FFE0E0FF")
    }
}
