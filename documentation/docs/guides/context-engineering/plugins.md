---
title: Plugins
sidebar_position: 6
sidebar_label: Plugins
---

# Plugins

Plugins are packages that extend ponduin with reusable components. A plugin can provide [skills](/docs/guides/context-engineering/using-skills), [hooks](/docs/guides/context-engineering/hooks), or both.

Use plugins when you want to install, share, or update a bundle of ponduin functionality instead of copying individual files into your local skills or hooks directories.

:::warning Install trusted plugins only
Plugins can include instructions that ponduin may load and hooks that execute local commands. Install plugins only from sources you trust, and review plugin contents before enabling them.
:::

## What Plugins Can Provide

| Component | What it does |
|---|---|
| Skills | Reusable instructions and supporting files that teach ponduin how to perform a task or follow a workflow. |
| Hooks | Local commands that run when lifecycle events happen during a ponduin session. |

A plugin is the container. Skills and hooks are components inside that container.

## Plugin Structure

A plugin is a directory with a plugin manifest and optional component directories. A plugin that includes both skills and hooks can look like this:

```text
my-plugin/
├── plugin.json
├── skills/
│   └── review/
│       └── SKILL.md
├── hooks/
│   └── hooks.json
└── scripts/
    └── notify.sh
```

The plugin manifest identifies the plugin:

```json title="plugin.json"
{
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "Reusable skills and hooks for my team"
}
```

### Add a Skill to a Plugin

To add a skill to a plugin, place a skill directory under the plugin's `skills/` directory. Each skill directory contains a `SKILL.md` file:

```text
my-plugin/
└── skills/
    └── review/
        └── SKILL.md
```

```markdown title="skills/review/SKILL.md"
---
name: review
description: Review code changes for correctness, maintainability, and test coverage
---

Review the code changes. Prioritize correctness issues, security concerns, missing tests, and maintainability risks. Be direct and suggest concrete fixes.
```

For Open Plugins, ponduin namespaces imported skill names with the plugin name. The `review` skill in `my-plugin` is loaded as `my-plugin:review`.

### Add a Hook to a Plugin

To add a hook to a plugin, create `hooks/hooks.json` and map lifecycle events to commands:

```text
my-plugin/
├── hooks/
│   └── hooks.json
└── scripts/
    └── notify.sh
```

```json title="hooks/hooks.json"
{
  "hooks": {
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "${PLUGIN_ROOT}/scripts/notify.sh"
          }
        ]
      }
    ]
  }
}
```

Hook commands receive the event payload as JSON on stdin. Use `${PLUGIN_ROOT}` to reference files inside the plugin directory.

For supported events, payload details, and more hook examples, see the [Hooks guide](/docs/guides/context-engineering/hooks).

## Plugin Locations

ponduin discovers plugins from these locations:

| Plugin type | Location | Notes |
|---|---|---|
| User plugin | `~/.agents/plugins/<plugin-name>/` | Includes plugins installed with `ponduin plugin install` and plugins manually copied into your user plugins directory. |
| Project plugin | `<project>/.agents/plugins/<plugin-name>/` | Available when ponduin is working in that project. |

Installed and manually placed user plugins use the same user plugins directory. Installed plugins include metadata created by `ponduin plugin install`; only installed git-backed plugins can be updated with `ponduin plugin update`.

## Install a Plugin

Install a plugin from a git repository with:

```bash
ponduin plugin install https://github.com/example/my-ponduin-plugin.git
```

The install command clones the repository, detects the plugin format, copies it into the plugins directory, and reports the imported components.

Example output:

```text
✓ Installed open-plugins plugin 'my-plugin' (1.0.0)
  Source: https://github.com/example/my-ponduin-plugin.git
  Location: /Users/you/.agents/plugins/my-plugin
  Imported skills:
    - my-plugin:review
    - my-plugin:test-plan
```

## Auto-Update a Plugin

To let ponduin check a plugin for updates automatically, install it with `--auto-update`:

```bash
ponduin plugin install --auto-update https://github.com/example/my-ponduin-plugin.git
```

When auto-update is enabled, ponduin checks that plugin for updates before plugin skills are loaded. Auto-update checks are rate-limited, so ponduin does not clone the repository on every session start.

If an auto-update fails, ponduin logs the failure and continues using the currently installed plugin.

:::note
Auto-update is available for git-backed plugins installed with `ponduin plugin install --auto-update`. Plugins copied manually into `.agents/plugins/` are discovered, but they are not managed by the plugin update command.
:::

## Update a Plugin Manually

To update a git-backed plugin on demand, run:

```bash
ponduin plugin update <plugin-name>
```

For example:

```bash
ponduin plugin update my-plugin
```

The update command fetches the plugin from its original git source, replaces the installed copy, and preserves whether auto-update was enabled for that plugin.

## Disable a Plugin

To disable a plugin globally, add its name to `disabledPlugins` in your user ponduin settings file:

```json title="~/.config/ponduin/settings.json"
{
  "disabledPlugins": ["my-plugin"]
}
```

For project-specific settings, use:

```text
<project>/.config/ponduin/settings.json
```

For local-only project settings that should not be shared with teammates, use:

```text
<project>/.config/ponduin/settings.local.json
```

A disabled plugin is skipped during plugin discovery, so its skills are not loaded and its hooks do not run.

## Plugin Formats

ponduin supports these plugin formats:

| Format | Common files | Notes |
|---|---|---|
| Open Plugins | `plugin.json`, `.plugin/plugin.json`, `.ponduin-plugin/plugin.json`, `skills/`, `hooks/hooks.json` | Supports Open Plugins skills and hooks. |
| Gemini extensions | `gemini-extension.json`, `skills/` | Supports skills from Gemini-style extension repositories. |

For Open Plugins, imported skill names are namespaced with the plugin name, such as `my-plugin:review`. Use that full name when explicitly loading a plugin-provided skill. Gemini extension skills keep the skill name from `SKILL.md`; ponduin does not prefix them with the extension name.

Open Plugins can use `plugin.json` at the plugin root, `.plugin/plugin.json`, or `.ponduin-plugin/plugin.json`. Hook-only Open Plugins can be discovered from `hooks/hooks.json`; if no manifest is present, ponduin infers the plugin name from the source or directory name.

## When to Use Plugins, Skills, or Hooks

| Use | Best fit |
|---|---|
| Package and distribute reusable ponduin components | Plugin |
| Teach ponduin a reusable procedure or domain-specific workflow | Skill |
| Run a local command when ponduin session events happen | Hook |

Plugins are for packaging and distribution. Skills and hooks define the behavior ponduin can use once the plugin is installed or discovered.
