package io.github.pondsec_ponduin.providers.databricks

public fun provider(host: String, token: String): io.github.pondsec_ponduin.Provider =
    io.github.pondsec_ponduin.databricksProvider(host, token)

public fun defaultModel(): String = io.github.pondsec_ponduin.databricksDefaultModel()
