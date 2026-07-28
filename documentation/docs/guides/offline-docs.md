---
title: Offline / Air-gapped Docs
sidebar_position: 95
sidebar_label: Offline Docs
---

# Offline / Air-gapped Docs

The `ponduin-doc-guide` skill reads official ponduin documentation before answering
ponduin-specific questions. By default it reads from `https://ponduin.de`. In
an offline or air-gapped environment, point ponduin at a **local copy** instead by
setting `PONDUIN_DOCS_ROOT`.

- If `PONDUIN_DOCS_ROOT` is set (in `config.yaml` or the environment), ponduin uses
  it as the docs root — either a local filesystem path or an HTTP(S) URL.
- If it is not set, ponduin falls back to `https://ponduin.de`.

When the root is a local path, ponduin reads the docs with its file tools; no
network access is required.

## Docs layout

A docs root contains a docs map and a `docs/` tree:

```
<docs-root>/
├── ponduin-docs-map.md
└── docs/
    ├── getting-started/...
    └── guides/...
```

`ponduin-docs-map.md` is the index the skill searches first; every page it reads
is referenced by a path listed there.

## Building a local docs root

Build the docs from a ponduin checkout using the same version as your ponduin
binary, so the docs match the runtime. The standard documentation build already
produces everything ponduin needs — a `ponduin-docs-map.md` index and a `docs/` tree
of markdown files — so no custom tooling is required:

```bash
git checkout v1.41.0   # match your ponduin binary version
cd documentation
npm run build
```

This writes the docs root to `documentation/build/`, containing:

```
build/
├── ponduin-docs-map.md
└── docs/
    ├── getting-started/...
    └── guides/...
```

`npm run build` requires registry access, so run it in an online environment.
Then copy the resulting `build/` directory to your air-gapped target location
(for example `/opt/ponduin-docs`) and point `PONDUIN_DOCS_ROOT` at it.

## Configuring ponduin

Set `PONDUIN_DOCS_ROOT` in `config.yaml`:

```yaml
PONDUIN_DOCS_ROOT: "/opt/ponduin-docs"
```

Or via the environment:

```bash
export PONDUIN_DOCS_ROOT=/opt/ponduin-docs
```

For a managed distribution, bake the docs tree into your image and set
`PONDUIN_DOCS_ROOT` in the shipped `config.yaml` or launcher environment.

## Notes

- Documentation links in ponduin's answers always render as canonical
  `https://ponduin.de/...` URLs, even when read locally.
- A custom HTTP(S) mirror also works: set `PONDUIN_DOCS_ROOT` to its root URL.
- For MCP extension runtime issues offline, see
  [Airgapped/Offline Environment Issues](/docs/troubleshooting/known-issues#airgappedoffline-environment-issues).
