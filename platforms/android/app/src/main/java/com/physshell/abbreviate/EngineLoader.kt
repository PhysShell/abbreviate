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

    fun fromAssets(assets: AssetManager): LoadedEngine =
        runCatching {
            val lexicon = readAsset(assets, "lexicon.tsv") ?: error("lexicon.tsv not bundled")
            val lm = readAsset(assets, "lm.tsv")
            val shortcuts = readAsset(assets, "shortcuts.tsv")
            LoadedEngine(UniffiSuggestionPort.fromData(lexicon, lm, shortcuts), lm != null, false)
        }.getOrElse {
            Log.w(TAG, "real lexicon unavailable, falling back to demo", it)
            LoadedEngine(UniffiSuggestionPort.demo(), hasLm = false, isDemo = true)
        }

    private fun readAsset(assets: AssetManager, name: String): String? =
        runCatching { assets.open(name).bufferedReader().use { it.readText() } }.getOrNull()
}
