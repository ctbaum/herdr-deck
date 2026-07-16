<p align="center">
  <img src="assets/herdr-deck-hero.png" alt="herdr-deck — one picker, your whole deck" width="100%">
</p>

# herdr-deck

An opinionated deck manager for [herdr](https://herdr.dev): one picker to browse,
launch, and tear down agent workspaces bound to git worktrees.

herdr-deck runs inside a herdr pane and drives everything by shelling out to the
`herdr` and [`wt` (worktrunk)](https://github.com/max-sixty/worktrunk) CLIs.
No daemon, no async runtime, one small binary.

> [!IMPORTANT]
> This is my personal workflow extracted into a public binary, not a generic
> Herdr workspace manager. The picker is reusable; the deck it builds is
> deliberately coupled to my nvim, agent, and terminal setup. Read
> [Requirements and compatibility](#requirements-and-compatibility) before
> installing.

```
╭─ herdr-deck [projects] ─╮╭──────────────────── dotfiles/main ────────────────────╮
│> dot▌             3 / 42││ nvim                         │ codex                  │
╰─────────────────────────╯│                              │                        │
╭──────── Results ────────╮│                              │                        │
│> ● dotfiles/main        ││──────────────────────────────┴────────────────────────│
│  ● dotfiles/ratatui-ux  ││ zsh                                                   │
│  ▸ ~/dotfiles.wt/...    ││                                                       │
╰─────────────────────────╯╰───────────────────────────────────────────────────────╯
```

## What it does

- **Browse**: live herdr workspaces first (agents blocked on you sort to the
  top), then worktrunk worktrees, then your zoxide directories. Type to
  filter. The preview shows a live 2D thumbnail of each workspace's actual
  pane layout, or worktree status (branch, merge state, dirty flags), or a
  directory listing.
- **Remotes**: set `HERDR_DECK_REMOTES` to a comma/space-separated list of ssh
  aliases and each remote herdr server becomes an entry (`⇄`); Enter opens a
  `herdr --remote` thin client in its own terminal window, leaving the local
  session alone (running it inside a pane would nest herdr in herdr).
- **Open**: Enter on a live workspace focuses it. Enter on a worktree or
  directory opens a launch form: pick an agent (detected from your PATH,
  using herdr's known-agent list), optionally enter a branch or Worktrunk
  shortcut (`^`, `-`, `@`, `pr:N`, `mr:N`, or a PR/MR URL), and go.
  Worktrunk creates or resolves the checkout, runs its lifecycle hooks, and
  returns its path to herdr-deck. The resulting deck is an editor + agent pane
  + full-width terminal + lazygit tab.
- **Resume**: `ctrl-s` switches to a separate session-history source, so past
  conversations never pollute workspace/path search. Type searches the first
  prompt and project path; Tab filters by agent. Claude, Codex, and Pi sessions
  resume in a recreated deck rooted at the session's original directory.
  Cursor opens its native session picker in that deck because its CLI does
  not expose an enumerable local history store.
- **Create**: `ctrl-n` prompts for a new directory. A new worktree in an
  existing repo is just Enter on the repo plus the branch field.
- **Destroy**: `ctrl-d` closes a workspace, or removes a worktree — but only
  when its branch is merged (worktrunk's `integrated`/`empty` state); an
  unmerged worktree gets an explicit force-remove confirmation instead.
  Removed paths are purged from zoxide.
- **Clean up**: `ctrl-g` opens a separate cleanable-worktree source containing
  every integrated/empty linked worktree found through the known repositories.
  Clean entries show `✓`; integrated worktrees with staged, modified,
  untracked, renamed, or deleted files show `!` and are never included in
  batch removal. Filter the review list if desired, then `ctrl-x` removes all
  visible clean entries after one count-and-project confirmation. Every entry
  is revalidated immediately before removal and no force flags are used.

## Looking for something less opinionated?

Try [Herdr Navigator](https://github.com/thanhdat77/herdr-navigator). It is a
configurable Herdr plugin that fuzzy-searches workspaces, agents, projects,
sessions, remotes, directories, and actions. Sources can be disabled, custom
command/JSON integrations can be added without changing Rust, and missing
optional tools degrade quietly.

The distinction is mostly intent: Herdr Navigator helps you **jump to
anything**; herdr-deck recreates **my particular working deck** around the
thing you selected.

## Requirements and compatibility

### Minimum

- [Herdr](https://herdr.dev) 0.7.0 or newer, with `herdr-deck` launched from
  inside a Herdr session. It talks directly to Herdr's socket CLI and exits
  otherwise. Repository identity and linked-worktree state come from Herdr's
  native worktree API, with Git as a compatibility fallback.
- A Unix-like environment. The core local workflow is intended for macOS or
  Linux; shell command construction and agent discovery assume Unix paths and
  process behavior.
- `git` for repository detection, branch labels, and worktree-aware behavior.
- `nvim` for every deck launch. It is not optional: the editor pane always
  runs `nvim`.
- The CLI for whichever agent you select, available on `PATH`. Herdr's direct
  agent integrations are strongly recommended so status indicators work.

Rust and Cargo are needed only to build from source. The binary does not install
or modify Neovim plugins; the editor bridge is a separate conventional plugin.

### Feature dependencies

| feature | dependency | behavior when missing |
|---|---|---|
| linked-worktree create/remove | [worktrunk](https://github.com/max-sixty/worktrunk) (`wt`) with JSON output | ordinary directory decks still work; worktree actions and status do not |
| directory discovery | `zoxide` and/or `fd` | that source becomes sparse or empty |
| directory preview | `eza`, with `ls` fallback | falls back to plain `ls -la` |
| git tab | `lazygit` | the tab is still created, but its command fails |
| Claude deck | Claude Code CLI + [herdr-agents.nvim](https://github.com/ctbaum/herdr-agents.nvim) + claudecode.nvim | nvim opens, but Claude does not auto-start |
| Codex deck | Codex CLI + [herdr-agents.nvim](https://github.com/ctbaum/herdr-agents.nvim) + codex.nvim | nvim opens, but Codex does not auto-start |
| connection-true pane identity | `pgrep`, `ps` or Linux `/proc`, `grep`, `sed`, `tr`, `sh` | same-tab geometry remains as a startup fallback |
| saved sessions | agent-owned local history files | only histories found at the supported hardcoded locations appear |
| remote entries | macOS `open` + Ghostty | remote launch is unavailable on other terminals/platforms |

Saved-session discovery currently reads `~/.claude/projects`,
`~/.codex/sessions`, and `~/.pi/agent/sessions`. Cursor exposes discovery only
through its own picker, so herdr-deck opens `cursor-agent ls`. These are
agent-owned storage formats and may change underneath herdr-deck.

### Neovim integration

Claude and Codex are intentionally launched by their Neovim plugins, after the
editor-side IDE server is ready. The reusable bridge now lives in
[herdr-agents.nvim](https://github.com/ctbaum/herdr-agents.nvim), with
claudecode.nvim and codex.nvim declared through your normal plugin manager.
For lazy.nvim:

```lua
local inside_herdr = vim.env.HERDR_SOCKET_PATH
  and vim.env.HERDR_SOCKET_PATH ~= ""

return {
  {
    "ctbaum/herdr-agents.nvim",
    cond = inside_herdr,
    lazy = false,
    dependencies = {
      { "coder/claudecode.nvim", dependencies = { "folke/snacks.nvim" } },
      { "ishiooon/codex.nvim", dependencies = { "folke/snacks.nvim" } },
    },
    opts = {},
  },
}
```

The plugin manager owns installation, updates, pins, and removal. Existing
dependency checkouts are reused rather than duplicated. If those upstream
plugins already have specs in your configuration, keep one spec for each and
avoid calling their ordinary `setup()` inside Herdr—the bridge supplies the
terminal providers there. Outside Herdr, retain their normal configuration.

herdr-agents.nvim installs **no key mappings** and reserves no leader namespace.
It exposes the upstream `:ClaudeCode*` and `:Codex*` commands plus
`:ClaudeHerdrSendSelection` and `:ClaudeHerdrSendDiagnostics`; users bind only
what they want. Run `:checkhealth herdr-agents` for local diagnostics.

The active package supplies the complete sibling-pane
behavior: external terminal providers, IDE environment forwarding, prompt
readiness waits, connection-port/process identity, same-tab fallback, agent
focus/send operations, selection and diagnostics forwarding, native diff
commands, and duplicate-agent protection. Connection-true identity inspects
local process environments using `pgrep` plus `ps`, or `/proc` on Linux; launch
arguments and IDE environment variables are forwarded into the new Herdr pane.

| variable | value set by herdr-deck |
|---|---|
| `HERDR_NVIM_AGENT` | `claude` or `codex` |
| `HERDR_NVIM_AGENT_ARGS_JSON` | JSON array containing the dangerous-mode flag when enabled and any saved-session resume arguments |

This ordering is intentional: both plugins create an editor-side server and
pass connection variables to the agent process. Starting the CLI independently
at the same time as nvim introduces a race and can leave the agent running with
no IDE connection. The binary only sets this launch contract; all editor-side
behavior belongs to herdr-agents.nvim.

The shell-readiness match defaults to `➜`. If your prompt does not contain that
symbol, set `HERDR_NVIM_PROMPT_MATCH` to stable text from your prompt; the agent
still launches after an eight-second timeout if it never matches.

### Environment variables

| variable | direction | purpose |
|---|---|---|
| `HERDR_NVIM_AGENT`, `HERDR_NVIM_AGENT_ARGS_JSON` | herdr-deck → workspace | launcher-neutral editor-agent startup contract described above |
| `HERDR_DECK_REMOTES` | user → herdr-deck | comma/space-separated SSH aliases shown as remote entries |
| `HERDR_NVIM_PROMPT_MATCH` | user → Neovim adapter | shell-prompt text awaited before launching the agent; defaults to `➜` |
| `HERDR_NAV_PASSTHROUGH_RE` | user → navigation plugin | lets `ctrl-j/k` reach herdr-deck when using seamless pane navigation |
| `HERDR_*` | Herdr → processes | inherited session/socket identity; scrubbed only when opening a remote Ghostty window |

### Safety defaults

The launch form starts with **dangerous mode enabled**. Claude, Codex, Cursor,
Devin, Droid, Kimi, OpenCode, Kilo, Hermes, Qoder CLI, and other known agents
receive their built-in bypass/yolo flag or environment override when one is
known. Disable the toggle before launching to omit it. Review `src/ext.rs`
before using this on a machine or repository where that default is not
acceptable.

## Install

### Native Herdr plugin

Herdr can install the repository, build the release binary, and expose its
`herdr-deck.open` action:

```sh
herdr plugin install ctbaum/herdr-deck
```

Bind that action in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "herdr-deck.open"
description = "Open herdr-deck"
```

The plugin action uses Herdr's injected workspace context, so the temporary
picker pane starts at the workspace root. Plugin installation requires Cargo
because Herdr builds the Rust binary from source.

### Standalone binary

Install straight from GitHub (no clone needed):

```sh
cargo install --git https://github.com/ctbaum/herdr-deck
```

Or from a clone:

```sh
git clone https://github.com/ctbaum/herdr-deck
cd herdr-deck
cargo install --path .
```

Either standalone route puts the binary in `~/.cargo/bin` (make sure that's on
your `PATH`). Install herdr-agents.nvim with your Neovim plugin manager as
described above, then bind the binary directly:

```toml
[[keys.command]]
key = "prefix+o"
type = "pane"
command = "herdr-deck"
```

Both installation modes run the same binary. herdr-deck exits when its pane
loses focus, so the temporary pane never sticks around.

## Keys

| key | action |
|-----|--------|
| type | filter (esc clears) |
| `↵` | focus workspace / open remote window / launch form / resume session |
| `ctrl-s` | switch projects / past sessions source |
| `ctrl-g` | toggle cleanable integrated-worktree source |
| `tab` / `shift-tab` | sessions: cycle agent filter |
| `ctrl-n` | new directory, then launch form |
| `ctrl-d` | close workspace / merge-gated worktree remove |
| `ctrl-x` | cleanable source: remove all visible clean entries |
| `ctrl-r` | reload |
| `ctrl-j/k` | move selection (needs passthrough, see below) |
| `?` | help |
| `esc` | back / quit |

If you use a herdr Ctrl-hjkl pane-navigation plugin (e.g.
vim-herdr-navigation), add `herdr-deck` to its passthrough list so `ctrl-j/k`
reach the picker:

```sh
export HERDR_NAV_PASSTHROUGH_RE='^(lazygit|herdr-deck)$'
```

## Opinionated setup and compatibility

herdr-deck mirrors my own personal workflow and layout:

- the deck layout is fixed: editor top-left, agent top-right, terminal
  bottom, lazygit on a new unfocused tab;
- `claude` and `codex` are special-cased to start through nvim and their IDE
  plugins, using the environment contract above; I plan to add pi and open code next.
- the "dangerous" toggle knows each agent's own yolo mechanism (flag or env)
  from a built-in table, and is **on by default**; unknown agents get no toggle;
- remote entries spawn their window via macOS `open` + Ghostty, hardcoded.

In practical terms: without nvim, deck launch is broken; without lazygit, the
git tab is empty; without herdr-agents.nvim, Claude/Codex decks contain only nvim;
and selecting an agent normally launches it with its unsafe/yolo mode enabled.
These are current design choices, not graceful optional paths.

Worktree discovery is structural—any directory whose `.git` is a file—so any
Worktrunk `worktree-path` layout works. Once selected, Worktrunk's JSON result
is authoritative for the checkout path and Herdr's native worktree metadata is
authoritative for repository identity. Removing a checkout also closes any
dedicated Herdr workspace rooted there; mixed workspaces lose only panes rooted
inside the removed checkout.

These constraints are part of herdr-deck's current opinionated scope. Issues
describing broader workflows are welcome, but configurability is not
guaranteed.
