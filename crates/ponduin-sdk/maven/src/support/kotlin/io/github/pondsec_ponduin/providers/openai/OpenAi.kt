package io.github.pondsec_ponduin.providers.openai

public fun provider(apiKey: String): io.github.pondsec_ponduin.Provider = io.github.pondsec_ponduin.openaiProvider(apiKey)

public fun defaultModel(): String = io.github.pondsec_ponduin.openaiDefaultModel()
