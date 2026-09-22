#!/bin/sh
set -eu

scene=${1:?usage: capture-scene.sh 1|2|3|4|5|6|7}
base=/Users/Shared/herdr-deck-demo
demo="$base/bin/demo-env"
session=herdr-deck-demo
recordings="$base/recordings"
discarded="$base/discarded/wrong-window-20260724"
recording_id=
window_id=$(
  cap targets windows --json |
    jq -r '[.[] | select(.ownerName == "Ghostty" and .name == "review-demo")] | last | .id'
)

demo_herdr() {
  "$demo" herdr --session "$session" "$@"
}

picker_workspace=$(
  demo_herdr workspace list |
    jq -r '.result.workspaces[] | select(.label == "picker/main") | .workspace_id'
)
picker_pane="$picker_workspace:p1"

wait_for() {
  pane=$1
  pattern=$2
  for _ in $(seq 1 15); do
    if demo_herdr pane read "$pane" --source visible --format text | rg -q "$pattern"; then
      return 0
    fi
    sleep 1
  done
  echo "timed out waiting for $pattern in $pane" >&2
  exit 1
}

archive_scene() {
  name=$1
  if [ -d "$recordings/$name.cap" ]; then
    mkdir -p "$discarded"
    target="$discarded/$name.cap"
    if [ -e "$target" ]; then
      target="$discarded/$name-$(date +%H%M%S).cap"
    fi
    mv "$recordings/$name.cap" "$target"
  fi
}

raise_window() {
  osascript \
    -e 'tell application "System Events"' \
    -e 'repeat with p in (every application process whose name is "ghostty")' \
    -e 'if exists window "review-demo" of p then' \
    -e 'set frontmost of p to true' \
    -e 'perform action "AXRaise" of window "review-demo" of p' \
    -e 'exit repeat' \
    -e 'end if' \
    -e 'end repeat' \
    -e 'end tell'
}

start_scene() {
  name=$1
  archive_scene "$name"
  raise_window
  sleep 1
  response=$(
    cap record start \
      --window "$window_id" \
      --mode studio \
      --fps 60 \
      --path "$recordings/$name.cap" \
      --json \
      --detach
  )
  recording_id=$(printf '%s\n' "$response" | jq -r 'select(.type == "started") | .recordingId')
  test -n "$recording_id"
  test "$recording_id" != "null"
  sleep 1
}

stop_scene() {
  sleep 1
  cap record stop --id "$recording_id" --json >/dev/null
  recording_id=
}

cleanup_recording() {
  if [ -n "$recording_id" ]; then
    cap record stop --id "$recording_id" --json >/dev/null 2>&1 || true
  fi
}

trap cleanup_recording EXIT INT TERM

case "$scene" in
  1)
    demo_herdr workspace focus "$picker_workspace" >/dev/null
    demo_herdr pane send-keys "$picker_pane" ctrl+c
    sleep 1
    demo_herdr pane send-text "$picker_pane" 'clear && herdr-deck'
    demo_herdr pane send-keys "$picker_pane" enter
    wait_for "$picker_pane" 'j/k move'
    start_scene scene-01-browse
    sleep 3
    demo_herdr pane send-keys "$picker_pane" down
    sleep 4
    demo_herdr pane send-keys "$picker_pane" down
    sleep 4
    stop_scene
    ;;
  2)
    demo_herdr workspace focus "$picker_workspace" >/dev/null
    demo_herdr pane send-keys "$picker_pane" ctrl+c
    sleep 1
    demo_herdr pane send-text "$picker_pane" 'clear && herdr-deck'
    demo_herdr pane send-keys "$picker_pane" enter
    wait_for "$picker_pane" 'j/k move'
    start_scene scene-02-launch
    demo_herdr pane send-keys "$picker_pane" /
    demo_herdr pane send-text "$picker_pane" projects/orbit
    sleep 3
    wait_for "$picker_pane" 'dev'
    demo_herdr pane send-keys "$picker_pane" enter
    sleep 2
    demo_herdr pane send-keys "$picker_pane" right
    sleep 1
    demo_herdr pane send-keys "$picker_pane" tab right tab
    demo_herdr pane send-text "$picker_pane" preview
    sleep 3
    demo_herdr pane send-keys "$picker_pane" down enter
    sleep 1
    demo_herdr pane send-keys "$picker_pane" tab space
    sleep 3
    demo_herdr pane send-keys "$picker_pane" enter
    sleep 4
    stop_scene
    ;;
  3)
    demo_herdr tab focus w3:t1 >/dev/null
    start_scene scene-03-deck
    sleep 5
    demo_herdr tab focus w3:t2 >/dev/null
    sleep 5
    demo_herdr tab focus w3:t1 >/dev/null
    sleep 5
    stop_scene
    ;;
  4)
    demo_herdr workspace focus w2 >/dev/null
    sleep 1
    demo_herdr workspace focus w3 >/dev/null
    sleep 1
    start_scene scene-04-toggle
    demo_herdr plugin action invoke toggle-project --plugin herdr-deck >/dev/null
    sleep 5
    demo_herdr plugin action invoke toggle-project --plugin herdr-deck >/dev/null
    sleep 5
    stop_scene
    ;;
  5)
    demo_herdr workspace focus "$picker_workspace" >/dev/null
    demo_herdr pane send-keys "$picker_pane" ctrl+c
    sleep 1
    demo_herdr pane send-text "$picker_pane" 'clear && herdr-deck'
    demo_herdr pane send-keys "$picker_pane" enter
    wait_for "$picker_pane" 'j/k move'
    demo_herdr pane send-keys "$picker_pane" ctrl+s
    wait_for "$picker_pane" 'Add a merge-safe cleanup confirmation'
    start_scene scene-05-sessions
    sleep 4
    demo_herdr pane send-keys "$picker_pane" tab
    sleep 4
    demo_herdr pane send-keys "$picker_pane" tab
    sleep 4
    stop_scene
    ;;
  6)
    demo_herdr workspace focus "$picker_workspace" >/dev/null
    demo_herdr pane send-keys "$picker_pane" ctrl+g
    wait_for "$picker_pane" 'orbit/cleanup-ready'
    start_scene scene-06-cleanup
    sleep 4
    demo_herdr pane send-keys "$picker_pane" down
    sleep 3
    demo_herdr pane send-keys "$picker_pane" ctrl+x
    wait_for "$picker_pane" 'remove 1 clean integrated worktree'
    sleep 4
    demo_herdr pane send-keys "$picker_pane" esc
    sleep 2
    stop_scene
    ;;
  7)
    demo_herdr workspace focus "$picker_workspace" >/dev/null
    demo_herdr pane send-keys "$picker_pane" ctrl+c
    sleep 1
    demo_herdr pane send-text "$picker_pane" 'clear && herdr-deck'
    demo_herdr pane send-keys "$picker_pane" enter
    wait_for "$picker_pane" 'j/k move'
    demo_herdr pane send-keys "$picker_pane" /
    demo_herdr pane send-text "$picker_pane" unmerged
    wait_for "$picker_pane" 'unmerged-lab'
    start_scene scene-07-safety
    sleep 3
    demo_herdr pane send-keys "$picker_pane" ctrl+d
    wait_for "$picker_pane" 'not merged'
    sleep 4
    demo_herdr pane send-keys "$picker_pane" esc
    sleep 2
    demo_herdr pane send-keys "$picker_pane" ctrl+n
    # herdr-deck expands this literal tilde.
    # shellcheck disable=SC2088
    demo_herdr pane send-text "$picker_pane" '~/projects/new-console'
    wait_for "$picker_pane" 'new-console'
    sleep 4
    demo_herdr pane send-keys "$picker_pane" esc
    sleep 2
    stop_scene
    ;;
  *)
    echo "unknown scene: $scene" >&2
    exit 1
    ;;
esac

printf 'Recorded scene %s with window %s\n' "$scene" "$window_id"
