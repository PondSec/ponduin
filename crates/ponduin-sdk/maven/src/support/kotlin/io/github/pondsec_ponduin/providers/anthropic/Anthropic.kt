package io.github.pondsec_ponduin.providers.anthropic

public fun provider(
    apiKey: String,
    baseUrl: String? = null,
    betaHeaders: List<String> = emptyList(),
): io.github.pondsec_ponduin.Provider = io.github.pondsec_ponduin.anthropicProvider(apiKey, baseUrl, betaHeaders)

public fun defaultModel(): String = io.github.pondsec_ponduin.anthropicDefaultModel()
