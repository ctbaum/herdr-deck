# wt-cockpit

An opinionated cockpit manager for [herdr](https://herdr.dev): one picker to browse,
launch, and tear down agent workspaces bound to git worktrees.

wt-cockpit runs inside a herdr pane and drives everything by shelling out to the
`herdr` and [`wt` (worktrunk)](https://github.com/max-sixty/worktrunk) CLIs.
No daemon, no async runtime, one small binary.

```
╭───────── wt-cockpit ─────────╮╭─────────────  dotfiles/main ─────────────╮
│> dot▌                 3 / 42 ││ nvim                │ harness            │
╰──────────────────────────────╯│                     │                    │
╭────────── Results ───────────╮│                     │                    │
│> ● dotfiles/main             ││─────────────────────┴────────────────────│
│  ● dotfiles/ratatui-ux       ││ zsh                                      │
│  ▸ ~/dotfiles.wt/feature-x   │╰──────────────────────────────────────────╯
╰──────────────────────────────╯
```

## What it does

- **Browse**: live herdr workspaces first (agents blocked on you sort to the
  top), then worktrunk worktrees, then your zoxide directories. Type to
  filter. The preview shows a live 2D thumbnail of each workspace's actual
  pane layout, or worktree status (branch, merge state, dirty flags), or a
  directory listing.
- **Remotes**: set `WT_COCKPIT_REMOTES` to a comma/space-separated list of ssh
  aliases and each remote herdr server becomes an entry (`⇄`); Enter opens a
  `herdr --remote` thin client in its own terminal window, leaving the local
  session alone (running it inside a pane would nest herdr in herdr).
- **Open**: Enter on a live workspace focuses it. Enter on a worktree or
  directory opens a launch form: pick an agent (detected from your PATH,
  using herdr's known-agent list), optionally name a branch (the worktree is
  created via `wt switch` if needed), and go. wt-cockpit builds a cockpit
  workspace: editor + agent pane + full-width terminal + a lazygit tab.
- **Resume**: `ctrl-s` switches to a separate session-history source, so past
  conversations never pollute workspace/path search. Type searches the first
  prompt and project path; Tab filters by agent. Claude, Codex, and Pi sessions
  resume directly in the picker pane. Cursor hands off to its native session
  picker because its CLI does not expose an enumerable local history store.
- **Create**: `ctrl-n` prompts for a new directory. A new worktree in an
  existing repo is just Enter on the repo plus the branch field.
- **Destroy**: `ctrl-d` closes a workspace, or removes a worktree — but only
  when its branch is merged (worktrunk's `integrated`/`empty` state); an
  unmerged worktree gets an explicit force-remove confirmation instead.
  Removed paths are purged from zoxide.

## Requirements

- [herdr](https://herdr.dev) (wt-cockpit talks to its socket CLI; run it inside
  a herdr session)
- [worktrunk](https://github.com/max-sixty/worktrunk) for the worktree flows
- optional: `zoxide` and `fd` (directory sources), `eza` (nicer listings),
  `lazygit` (the git tab), `nvim` (the editor pane)

## Install

Straight from GitHub (no clone needed):

```sh
cargo install --git https://github.com/ctbaum/wt-cockpit
```

Or from a clone:

```sh
git clone https://github.com/ctbaum/wt-cockpit
cd wt-cockpit
cargo install --path .
```

Either way the binary lands in `~/.cargo/bin` (make sure that's on your
PATH). Then bind it in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+o"
type = "pane"
command = "wt-cockpit"
```

wt-cockpit exits when its pane loses focus, so the temporary pane never sticks
around.

## Keys

| key | action |
|-----|--------|
| type | filter (esc clears) |
| `↵` | focus workspace / open remote window / launch form / resume session |
| `ctrl-s` | switch projects / past sessions source |
| `tab` / `shift-tab` | sessions: cycle agent filter |
| `ctrl-n` | new directory, then launch form |
| `ctrl-d` | close workspace / merge-gated worktree remove |
| `ctrl-r` | reload |
| `ctrl-j/k` | move selection (needs passthrough, see below) |
| `?` | help |
| `esc` | back / quit |

If you use a herdr Ctrl-hjkl pane-navigation plugin (e.g.
vim-herdr-navigation), add `wt-cockpit` to its passthrough list so `ctrl-j/k`
reach the picker:

```sh
export HERDR_NAV_PASSTHROUGH_RE='^(tv|lazygit|wt-cockpit)$'
```

## Honesty section

wt-cockpit mirrors my own personal workflow and layout:

- the cockpit layout is fixed: editor top-left, agent top-right, terminal
  bottom, lazygit on a new unfocused tab;
- `claude` is special-cased to run *inside* nvim via a claudecode herdr
  provider (signalled with `NIC_AI`/`NIC_CLAUDE_ARGS` env vars that a nvim
  config has to pick up), and always with `--dangerously-skip-permissions`;
- the "dangerous" toggle knows each agent's own yolo mechanism (flag or env)
  from a built-in table, and is on by default; unknown agents get no toggle;
- remote entries spawn their window via macOS `open` + Ghostty, hardcoded.

Worktrees are recognised structurally — any directory whose `.git` is a
file (a linked git worktree) — so any worktrunk `worktree-path` layout
works.

If any of these bite you, open an issue — making them configurable is the
obvious next step.
