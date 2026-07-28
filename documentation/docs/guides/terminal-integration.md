# Terminal Integration

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';

Talk to ponduin directly from your shell prompt. Instead of switching to a separate REPL session, stay in your terminal and call ponduin when you need it.

## Setup

<Tabs groupId="shells">
<TabItem value="zsh" label="zsh" default>

Add to `~/.zshrc`:
```bash
eval "$(ponduin term init zsh)"
```

</TabItem>
<TabItem value="bash" label="bash">

Add to `~/.bashrc`:
```bash
eval "$(ponduin term init bash)"
```

</TabItem>
<TabItem value="fish" label="fish">

Add to `~/.config/fish/config.fish`:
```fish
ponduin term init fish | source
```

</TabItem>
<TabItem value="nu" label="Nushell">

Add to `~/.config/nushell/config.nu`:
```nu
let ponduin_term_init = ($nu.cache-dir | path join "ponduin-term-init.nu")
^ponduin term init nu | save --force $ponduin_term_init
source $ponduin_term_init
```

</TabItem>
<TabItem value="powershell" label="PowerShell">

Add to `$PROFILE`:
```powershell
Invoke-Expression (ponduin term init powershell)
```

</TabItem>
</Tabs>

Restart your terminal or source the config, and that's it!

## Usage

Just type `@ponduin` (or `@g` for short) followed by your question:

```bash
npm install express
    npm ERR! code EACCES
    npm ERR! permission denied

@ponduin "how do I fix this error?"
```

ponduin automatically sees the commands you've run since your last question, so you don't need to explain what you've been doing. Use quotes around your prompt if it contains special characters like `?`, `*`, or `'`:

```bash
@ponduin "what's in this directory?"
@g "analyze the error: 'permission denied'"
```

## Named Sessions
By default, each terminal gets its own ponduin session that lasts until you close it. Named sessions let you continue conversations across terminal restarts and share context between windows.

<Tabs groupId="shells">
<TabItem value="zsh" label="zsh" default>

```bash
eval "$(ponduin term init zsh --name my-project)"
```

</TabItem>
<TabItem value="bash" label="bash">

```bash
eval "$(ponduin term init bash --name my-project)"
```

</TabItem>
<TabItem value="fish" label="fish">

```fish
ponduin term init fish --name my-project | source
```

</TabItem>
<TabItem value="nu" label="Nushell">

```nu
let ponduin_term_init = ($nu.cache-dir | path join "ponduin-term-init.nu")
^ponduin term init nu --name my-project | save --force $ponduin_term_init
source $ponduin_term_init
```

</TabItem>
<TabItem value="powershell" label="PowerShell">

```powershell
Invoke-Expression (ponduin term init powershell --name my-project)
```

</TabItem>
</Tabs>

Named sessions persist in ponduin's database, so they're available anytime, even after restarting your computer. Reopen later and run the same command to continue:

```bash
# Start debugging
eval "$(ponduin term init zsh --name auth-bug)"
@ponduin help me debug this login timeout

# Close terminal, come back later
eval "$(ponduin term init zsh --name auth-bug)"
@ponduin "what was the solution we discussed?"
# Continues the same conversation with context
```

## Default Handler

Use `--default` if you want ponduin to answer commands your shell cannot resolve.

<Tabs groupId="default-shells">
<TabItem value="zsh" label="zsh" default>

```bash
eval "$(ponduin term init zsh --default)"
```

</TabItem>
<TabItem value="bash" label="bash">

```bash
eval "$(ponduin term init bash --default)"
```

</TabItem>
<TabItem value="nu" label="Nushell">

```nu
let ponduin_term_init = ($nu.cache-dir | path join "ponduin-term-init.nu")
^ponduin term init nu --default | save --force $ponduin_term_init
source $ponduin_term_init
```

</TabItem>
</Tabs>

## Show Context Status in Your Prompt

Add `ponduin term info` to your prompt to see how much context you've used and which model is active during a terminal ponduin session.

<Tabs groupId="shells">
<TabItem value="zsh" label="zsh" default>

```bash
PROMPT='$(ponduin term info) %~ $ '
```

</TabItem>
<TabItem value="bash" label="bash">

```bash
PS1='$(ponduin term info) \w $ '
```

</TabItem>
<TabItem value="fish" label="fish">

```fish
function fish_prompt
    ponduin term info
    echo -n ' '(prompt_pwd)' $ '
end
```

</TabItem>
<TabItem value="nu" label="Nushell">

```nu
$env.PROMPT_COMMAND = {|| $"(ponduin term info) (pwd)> " }
```

</TabItem>
<TabItem value="powershell" label="PowerShell">

```powershell
function prompt {
    $ponduinInfo = & ponduin term info
    "$ponduinInfo $(Get-Location) PS> "
}
```

</TabItem>
</Tabs>

Your terminal prompt now shows the context usage and model name (shortened for readability) for the active ponduin session. For example:

```bash
●●○○○ sonnet ~/projects $
```
## Shell Completion for ponduin Commands

`@ponduin` provides context-aware assistance based on your command history. To enable tab completion of ponduin CLI commands (like `ponduin session`, `ponduin run`, etc.), see the [shell completion documentation](/docs/guides/ponduin-cli-commands#completion).

## Troubleshooting

**ponduin doesn't see recent commands:**
If you run commands but ponduin says it doesn't see any recent activity, check if terminal integration is properly [set up in your shell config](#setup).
You can also check the id of the ponduin session in your current terminal:
```bash
# Check if session ID exists
echo $AGENT_SESSION_ID
# Should show something like: 20251209_151730
```
```nu
# Nushell
$env.AGENT_SESSION_ID
# Should show something like: 20251209_151730
```
To share context across terminal windows, use a [named session](#named-sessions) instead.

**Session getting too full** (prompt shows `●●●●●`):
If ponduin's responses are getting slow or hitting context limits, start a fresh ponduin session in the terminal. The new ponduin session sees your command history, but not the conversation history from the previous session.
```bash
# Start a new ponduin session in the same shell
eval "$(ponduin term init zsh)"
```
```nu
# Nushell
let ponduin_term_init = ($nu.cache-dir | path join "ponduin-term-init.nu")
^ponduin term init nu | save --force $ponduin_term_init
source $ponduin_term_init
```
