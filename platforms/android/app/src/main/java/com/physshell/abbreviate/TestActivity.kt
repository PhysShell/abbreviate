package com.physshell.abbreviate

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.Editable
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.TextWatcher
import android.text.style.ForegroundColorSpan
import android.util.TypedValue
import android.view.Gravity
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Typing-speed tester. Shows a target line, counts down 3-2-1, times the run
 * from the first keystroke, and auto-stops when the input matches the target.
 *
 * It is keyboard-agnostic on purpose: whatever IME is currently selected does
 * the typing, so you compare "обычная клавиатура" vs the abbreviation IME by
 * switching the active keyboard between runs. Each result is tagged with the
 * active IME. Time / CPM / WPM and an edit count (a keystroke proxy — taps
 * inside another keyboard's process aren't observable) are recorded; the
 * richer per-tap/per-suggestion metrics live in the web tester.
 */
class TestActivity : Activity() {

    private lateinit var target: TextView
    private lateinit var editor: EditText
    private lateinit var countdown: TextView
    private lateinit var result: TextView
    private lateinit var history: LinearLayout
    private val main = Handler(Looper.getMainLooper())

    private var targetText = ""
    private var startedAt = 0L
    private var edits = 0
    private var running = false
    private var applying = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(BG)
            setPadding(dp(20), dp(24), dp(20), dp(24))
        }

        root.addView(
            TextView(this).apply {
                text = "Тест набора"
                setTextColor(INK)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 22f)
                setTypeface(typeface, Typeface.BOLD)
                setPadding(0, 0, 0, dp(4))
            },
        )
        root.addView(
            TextView(this).apply {
                text = "Наберите текст на скорость — таймер сам остановится при совпадении. " +
                    "Сравните: переключите клавиатуру и пройдите ещё раз."
                setTextColor(MUTED)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
                setPadding(0, 0, 0, dp(12))
            },
        )

        target = TextView(this).apply {
            background = panel()
            setTextColor(INK)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
            setPadding(dp(14), dp(12), dp(14), dp(12))
        }
        root.addView(target, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { bottomMargin = dp(8) })

        countdown = TextView(this).apply {
            gravity = Gravity.CENTER
            setTextColor(ACCENT)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 40f)
            visibility = TextView.GONE
        }
        root.addView(countdown, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))

        editor = EditText(this).apply {
            hint = "Нажмите «Новый текст», затем печатайте…"
            setLines(3)
            gravity = Gravity.TOP or Gravity.START
            setTextColor(INK)
            setHintTextColor(MUTED)
            background = panel()
            setPadding(dp(14), dp(12), dp(14), dp(12))
            isEnabled = false
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                override fun afterTextChanged(s: Editable?) {
                    if (!applying) onTyped()
                }
            })
        }
        root.addView(editor, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))

        result = TextView(this).apply {
            setTextColor(INK)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            setPadding(0, dp(10), 0, 0)
            visibility = TextView.GONE
        }
        root.addView(result)

        root.addView(
            Button(this).apply {
                text = "Новый текст"
                isAllCaps = false
                setOnClickListener { newTest() }
            },
            LinearLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT).apply { topMargin = dp(10) },
        )

        root.addView(
            TextView(this).apply {
                text = "Последние результаты"
                setTextColor(MUTED)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
                setPadding(0, dp(16), 0, dp(4))
            },
        )
        history = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(history)

        setContentView(root)
        renderTarget("")
    }

    private fun newTest() {
        targetText = TARGETS.random()
        edits = 0
        running = false
        startedAt = 0L
        result.visibility = TextView.GONE
        applying = true
        editor.setText("")
        applying = false
        editor.isEnabled = false
        renderTarget("")
        tick(3)
    }

    private fun tick(n: Int) {
        countdown.visibility = TextView.VISIBLE
        if (n > 0) {
            countdown.text = n.toString()
            main.postDelayed({ tick(n - 1) }, 700)
        } else {
            countdown.text = "Печатай!"
            main.postDelayed({
                countdown.visibility = TextView.GONE
                editor.isEnabled = true
                editor.requestFocus()
            }, 500)
        }
    }

    private fun onTyped() {
        if (targetText.isEmpty()) return
        if (!running) {
            running = true
            startedAt = System.nanoTime()
        }
        edits++
        renderTarget(editor.text.toString())
        if (norm(editor.text.toString()) == norm(targetText)) finish_()
    }

    private fun finish_() {
        running = false
        editor.isEnabled = false
        val seconds = (System.nanoTime() - startedAt) / 1_000_000_000.0
        val chars = targetText.length
        val words = targetText.trim().split(Regex("\\s+")).size
        val cpm = if (seconds > 0) Math.round(chars / seconds * 60) else 0
        val wpm = if (seconds > 0) Math.round(words / seconds * 60) else 0

        val line = "%.1f с · %d зн/мин · %d сл/мин · %d правок · [%s]".format(seconds, cpm, wpm, edits, activeKeyboard())
        result.text = line
        result.visibility = TextView.VISIBLE
        history.addView(
            TextView(this).apply {
                text = "• $line"
                setTextColor(MUTED)
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
                setPadding(0, dp(2), 0, dp(2))
            },
            0,
        )
    }

    /** Short label for the currently selected input method (to tag the run). */
    private fun activeKeyboard(): String {
        val id = Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD).orEmpty()
        return when {
            id.startsWith(packageName) -> "наша"
            id.isEmpty() -> "?"
            else -> id.substringBefore('/').substringAfterLast('.')
        }
    }

    private fun renderTarget(typed: String) {
        var i = 0
        while (i < typed.length && i < targetText.length && typed[i] == targetText[i]) i++
        val s = SpannableStringBuilder(targetText)
        if (i > 0) s.setSpan(ForegroundColorSpan(ACCENT), 0, i, Spanned.SPAN_INCLUSIVE_EXCLUSIVE)
        target.text = s
    }

    private fun panel() = GradientDrawable().apply {
        setColor(PANEL)
        cornerRadius = dp(10).toFloat()
        setStroke(dp(1), BORDER)
    }

    private fun dp(v: Int): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    private fun norm(s: String): String = s.replace(Regex("\\s+"), " ").trim().lowercase()

    companion object {
        private val TARGETS = listOf(
            "привет как дела сегодня",
            "я работаю над новым проектом",
            "давай встретимся завтра вечером",
            "спасибо за помощь с задачей",
            "это было очень интересно и полезно",
            "мы пойдём в кино в субботу",
            "надо подумать над этим решением",
            "всё хорошо не беспокойся обо мне",
            "увидимся на следующей неделе",
            "какой у нас план на сегодня",
        )

        private val BG = Color.parseColor("#16181D")
        private val PANEL = Color.parseColor("#1F2229")
        private val INK = Color.parseColor("#E8EAED")
        private val MUTED = Color.parseColor("#9AA0A6")
        private val ACCENT = Color.parseColor("#6EA8FE")
        private val BORDER = Color.parseColor("#2C303A")
    }
}
