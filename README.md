<p align="center">
  <img src="assets/herdr-deck-hero.png" alt="herdr-deck: one picker, your whole deck" width="100%">
</p>

# herdr-deck

An opinionated workspace launcher for [Herdr](https://herdr.dev) and
[herdr-agents.nvim](https://github.com/ctbaum/herdr-agents.nvim). Pick a
project, Git worktree, or saved agent session and open a cockpit with Neovim,
a connected agent, and a shell.

> [!IMPORTANT]
> This is my personal workflow, not a generic workspace manager. The picker is
> reusable; the cockpit layout, editor integration, and remote launcher are
> deliberately fixed.

## Demo

<video src="https://github.com/user-attachments/assets/76b12347-9811-4f1f-902d-ff5972f6cb67" controls muted width="100%"></video>

## What it does

- **Browse** live Herdr workspaces, Git roots and worktrees, zoxide directories,
  and plain directories. `/` filters; previews show the live pane layout,
  worktree status, or directory contents.
- **Open** a workspace or launch a new cockpit for the same checkout, an
  existing worktree, or a new branch/PR/MR via
  [Worktrunk](https://github.com/max-sixty/worktrunk). `ctrl-o` adds a cockpit
  as a neighboring tab when possible.
- **Connect agents** to Neovim through herdr-agents.nvim. Ctrl-clicking an agent
  file link opens it in that cockpit's existing editor.
- **Resume sessions** from Claude, Codex, and Pi. Cursor opens its own session
  picker because its CLI does not expose searchable history.
- **Manage worktrees** with merge-gated removal and batch cleanup of clean,
  integrated worktrees. Dirty worktrees are excluded and every removal is
  revalidated.
- **Open remotes** listed in `HERDR_DECK_REMOTES` as `herdr --remote` Ghostty
  windows.
- **Toggle projects** with the native `herdr-deck.toggle-project` action, which
  switches between the two most recently visited projects.
- **Survive Herdr restarts** by reattaching detached Neovim processes and
  resuming disconnected agents.

Want a configurable alternative? Try
[Herdr Navigator](https://github.com/thanhdat77/herdr-navigator).

## Requirements and compatibility

### Required

- [Herdr](https://herdr.dev) 0.7.5 or newer. `herdr-deck` must run inside a
  Herdr session.
- A Unix-like system, `git`, and `nvim`.
- The CLI for each agent you use. Herdr's agent integrations are recommended
  for status reporting.

Rust and Cargo are needed only to build or install from source.

### Opinionated setup

Each cockpit is one tab: editor top-left (70%), agent top-right (30%), and a
full-width shell at the bottom (20%). Git stays inside Neovim through Neogit.
The layout is fixed.

Claude, Codex, and Pi start through Neovim after their IDE servers are ready.
Pi also requires `pi.enabled = true`, pi-ide.nvim, and:

```sh
pi install npm:@ldelossa/pi-ide
```

Remote launch is hardcoded to macOS `open` + Ghostty.

Any directory with a `.git` file is treated as a worktree, so custom Worktrunk
`worktree-path` layouts work. Worktrunk decides the checkout path; Herdr decides
repository identity. Removing a checkout closes a workspace rooted entirely
inside it, or only matching panes in a mixed workspace.

### Optional dependencies

| feature | dependency | without it |
| --- | --- | --- |
| worktree create/remove | `wt` with JSON output | directory cockpits still work |
| directory discovery | `zoxide` and/or `fd` | fewer or no directory results |
| directory preview | `eza` | falls back to `ls -la` |
| Claude | Claude CLI, herdr-agents.nvim, claudecode.nvim | Neovim opens without Claude |
| Codex | Codex CLI, herdr-agents.nvim, codex.nvim | Neovim opens without Codex |
| Pi | Pi CLI, herdr-agents.nvim, pi-ide.nvim, Pi `pi-ide` extension | Pi does not auto-connect |
| agent process matching | `pgrep`, `ps` or Linux `/proc`, `grep`, `sed`, `tr`, `sh` | only stable-name recovery works |
| saved sessions | agent-owned history files | unavailable histories are omitted |
| remotes | macOS `open` + Ghostty | remote launch is unavailable |

Saved sessions are read from `~/.claude/projects`, `~/.codex/sessions`, and
`~/.pi/agent/sessions`. These agent-owned formats may change.

## Neovim integration

Install the bridge with your plugin manager. For lazy.nvim:

```lua
local inside_herdr = (vim.env.HERDR_SOCKET_PATH or "") ~= ""

return {
  {
    "ctbaum/herdr-agents.nvim",
    cond = inside_herdr,
    lazy = false,
    dependencies = {
      { "coder/claudecode.nvim", dependencies = { "folke/snacks.nvim" } },
      { "ishiooon/codex.nvim", dependencies = { "folke/snacks.nvim" } },
      { "ldelossa/pi-ide.nvim" },
    },
    opts = { pi = { enabled = true } },
  },
}
```

Keep one spec for each upstream plugin and do not call its usual `setup()`
inside Herdr; the bridge supplies the terminal providers. Outside Herdr, keep
its normal configuration.

herdr-agents.nvim installs no mappings. It exposes the upstream
`:ClaudeCode*` and `:Codex*` commands plus `:ClaudeHerdrSendSelection` and
`:ClaudeHerdrSendDiagnostics`. Run `:checkhealth herdr-agents` for diagnostics.

herdr-deck passes this launch contract to the editor:

| variable | purpose |
| --- | --- |
| `HERDR_NVIM_AGENT` | selected `claude`, `codex`, or `pi` adapter |
| `HERDR_NVIM_AGENT_ARGS_JSON` | dangerous-mode and session-resume arguments |
| `HERDR_NVIM_AGENT_RECOVER` | permit safe agent recovery after editor relaunch |
| `HERDR_NVIM_AGENT_RECOVER_WAIT_MS` | recovery delay; automatic restores use `5000` |
| `NVIM_LISTEN_ADDRESS` | preserve the cockpit's Neovim RPC endpoint |
| `HERDR_NVIM_AGENT_START_TIMEOUT` | agent startup timeout in milliseconds; default `30000` |
| `HERDR_DECK_REMOTES` | comma/space-separated SSH aliases |
| `HERDR_DECK_RUNTIME_DIR` | optional parent directory for Neovim sockets |

Starting an agent CLI independently alongside Neovim can race its IDE server;
let the bridge start it.

### Recovery

Neovim runs detached with a pane-scoped RPC socket. After a Herdr server
restart, the plugin reattaches the surviving process, preserving editor state,
and replaces any disconnected agent from its native session reference. If the
editor itself is gone, it starts a fresh Neovim; editor state is not restored.
A missing session reference leaves the existing agent untouched rather than
risking data loss or duplication.

### Safety

**Dangerous mode is enabled by default.** Known agents receive their bypass or
yolo flag. Disable it in the launch form when that is not acceptable.

## Install

### Native Herdr plugin

```sh
herdr plugin install ctbaum/herdr-deck
```

Bind its actions in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "herdr-deck.open"
description = "Open herdr-deck"

[[keys.command]]
key = "alt+o"
type = "plugin_action"
command = "herdr-deck.toggle-project"
description = "Toggle previous project"
```

The picker opens as a native popup using the current workspace context. Herdr
builds the plugin from source, so installation requires Cargo.

### Standalone binary

```sh
cargo install --git https://github.com/ctbaum/herdr-deck
```

Or from a clone:

```sh
git clone https://github.com/ctbaum/herdr-deck
cd herdr-deck
cargo install --path .
```

Then bind the binary directly:

```toml
[[keys.command]]
key = "prefix+o"
type = "pane"
command = "herdr-deck"
```

Both modes run the same binary. The plugin uses a popup; standalone mode uses
the current pane and exits when it loses focus.

## Controls

Mouse hover previews, click opens, and the wheel scrolls lists. Tabs, header
actions, forms, dialogs, and help are clickable; clicking outside a dialog
cancels it. Colors follow Herdr's configured theme. Automatic theme switching
uses the configured dark theme because plugins cannot read the live host
appearance.

| key | action |
| --- | --- |
| `j` / `k` | move through results |
| `h` / `l` | previous / next source |
| `g` / `G` | first / last result |
| `1` / `2` / `3` | projects / sessions / cleanable worktrees |
| `/` | search; `esc` returns to navigation |
| `↑` / `↓` or `ctrl-j/k` | move while searching |
| `↵` | focus, open, launch, or resume |
| `ctrl-o` | new cockpit for the selected item |
| `ctrl-s` / `ctrl-g` | sessions / cleanable worktrees |
| `tab` / `shift-tab` | cycle session agent filter |
| `ctrl-n` | create a directory, then launch |
| `ctrl-d` | close workspace or remove worktree |
| `ctrl-x` | remove all visible clean worktrees |
| `ctrl-r` | reload |
| `?` | help |
| `q` | close |
| `esc` | leave search, clear search, or close |

In launch forms, `j/k` changes fields and `h/l` changes values. The worktree
field is text input; arrows or `ctrl-j/k` move through its candidates.
