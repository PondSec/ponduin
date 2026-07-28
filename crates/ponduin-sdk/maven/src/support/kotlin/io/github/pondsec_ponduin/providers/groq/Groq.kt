package io.github.pondsec_ponduin.providers.groq

public fun provider(apiKey: String): io.github.pondsec_ponduin.Provider = io.github.pondsec_ponduin.groqProvider(apiKey)

public fun defaultModel(): String = io.github.pondsec_ponduin.groqDefaultModel()
