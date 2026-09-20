# Herdr Deck showcase script

Target runtime: 1:49

Delivery: 3840 × 2160, 60 fps, H.264, silent with on-screen captions

Capture style: direct action, no hero shot

## Performance rule

Treat every remote input as a cue, not a flourish. Send one input, hold for the
interface response, then let the result remain readable before sending the next
input. The capture timings include 1 to 5 seconds of visual hold around each
state change. Do not compensate for control lag with repeated keys or mouse
movement.

## Shot list

| Time | Picture and action | On-screen copy | Lag allowance |
| --- | --- | --- | --- |
| 00:00–00:13 | Begin inside the open project picker. Move through Atlas, Nebula, and Orbit so the project grouping, blocked state, and live layout preview are immediately visible. | “Blocked agents rise to the top.” Then: “Browse by project, with the live layout in view.” | Hold each selection long enough for the preview to settle. |
| 00:13–00:32 | Filter to the neutral Orbit fixture. Open the launch form, choose **worktree**, choose Codex, enter `feature/preview`, and launch. | “Choose the same checkout or a worktree, then pick the agent.” Then: “Worktrunk resolves the checkout, then Herdr launches it.” | Pause after filtering, opening the form, and changing fields. Leave a final hold for launch feedback. |
| 00:32–00:50 | Show the resulting single-tab workspace: editor, agent pane, and shell. Move focus through the three panes, then return to the editor. | “Editor, agent, and shell in one workspace.” Then: “One cockpit keeps everything in view.” | Hold each pane long enough for the focus change to read clearly. |
| 00:50–01:02 | Invoke quick toggle once to move from Orbit to Nebula, hold, then invoke it again to return to Orbit. | “Jump between your two most recent projects.” | One command per cut, followed by a five-second confirmation hold. |
| 01:02–01:16 | Open saved-session history. Show the combined list, then cycle the agent filter through Claude and Codex. | “Resume saved conversations in project context.” Then: “Filter session history by agent.” | Four-second holds after each filter change. |
| 01:16–01:31 | Open cleanup mode. Select the clean integrated worktree while leaving the dirty one marked for exclusion. Open the confirmation, hold on its counts, then cancel. | “Batch cleanup selects only clean, integrated worktrees.” Then: “Dirty work is identified and skipped.” | Wait for the confirmation text before the hold. Cancel only after it is fully readable. |
| 01:31–01:49 | Filter to the diverged worktree and request deletion. Hold on the explicit force gate, then cancel. Open the new-directory form, enter `~/projects/new-console`, hold, and cancel. Fade out from the live interface. | “Unmerged work stays protected behind an explicit force gate.” Then: “Create a new project directory without leaving the deck.” Final beat: “One picker. Your whole deck.” | Three to four seconds around each modal. Never send a destructive confirmation. |

## Privacy and safety continuity

The performance uses the named Herdr session `herdr-deck-demo` and an isolated
home under `/Users/Shared/herdr-deck-demo`. All repositories, Git identity,
shell history, configuration, and saved agent sessions are synthetic. The agent
pane is a static, non-networked demo surface against the neutral fixture. It
contains no account state and receives no prompt. Cleanup and deletion
confirmations are shown, then canceled.
