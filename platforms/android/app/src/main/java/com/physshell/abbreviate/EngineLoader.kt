package com.physshell.abbreviate

import android.content.res.AssetManager
import android.util.Log
import com.physshell.abbreviate.engine.UniffiSuggestionPort

/** Outcome of building the engine from bundled assets. */
data class LoadedEngine(val port: UniffiSuggestionPort, val hasLm: Boolean, val isDemo: Boolean)

/**
 * Builds the engine from the bundled TSV assets (lexicon + optional LM and
 * shortcuts), falling back to the demo lexicon if they're missing. Shared by
 * every shell (scratchpad, IME) so the asset names and fallback live in one
 * place. Blocking and heavy (parses ~11 MB and builds the index) — call it off
 * the main thread.
 */
object EngineLoader {
    private const val TAG = "Abbrev"

    // try/catch over Exception (not runCatching, which also swallows Errors like
    // OutOfMemoryError) — only recoverable failures fall back to the demo.
    fun fromAssets(assets: AssetManager): LoadedEngine =
        try {
            val lexicon = readAsset(assets, "lexicon.tsv") ?: error("lexicon.tsv not bundled")
            val lm = readAsset(assets, "lm.tsv")
            val shortcuts = readAsset(assets, "shortcuts.tsv")
            val mask = readAsset(assets, "mask.txt")
            val tone = readAsset(assets, "tone.tsv")
            LoadedEngine(
                UniffiSuggestionPort.fromData(lexicon, lm, shortcuts, mask, tone),
                lm != null,
                false,
            )
        } catch (e: Exception) {
            Log.w(TAG, "real lexicon unavailable, falling back to demo", e)
            LoadedEngine(UniffiSuggestionPort.demo(), hasLm = false, isDemo = true)
        }

    private fun readAsset(assets: AssetManager, name: String): String? =
        try {
            assets.open(name).bufferedReader().use { it.readText() }
        } catch (_: Exception) {
            null
        }
}
