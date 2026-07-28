# ponduin-sdk

The bindings layer for Ponduin. It houses the shared types used for both ACP and
SDK access, and exposes a cross-language version of the Ponduin API.

With `--features uniffi` the crate compiles to native bindings for Python and
Kotlin (namespace `ponduin` / `io.github.pondsec_ponduin`). The UniFFI surface lets
callers construct providers, stream provider completions, perform non-streaming
completion, and pass rich message/tool content across the FFI boundary.

```bash
just python   # build bindings + run examples/uniffi/provider.py
just kotlin   # build the Maven artifact + run examples/uniffi/kotlin
```

## Python package

The PyPI package is published as `ponduin-sdk` and imports as `ponduin`.
Build a local wheel from the repository root with:

```bash
just --justfile crates/ponduin-sdk/justfile python-wheel
```

This regenerates the UniFFI Python bindings, copies the release native library
into the package, and writes the wheel to `crates/ponduin-sdk/python/dist/`.

## Maven package

The Maven Central artifact is published as `io.github.PondSec:pdk` and uses
the Rust crate version from `crates/ponduin-sdk/Cargo.toml`.

```bash
just --justfile crates/ponduin-sdk/justfile maven-package
```

This regenerates the UniFFI Kotlin bindings and packages them with the native
library in a JVM jar. CI builds the native libraries for supported platforms and
can optionally publish the combined artifact to Maven Central.
