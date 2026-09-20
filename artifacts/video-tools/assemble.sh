#!/bin/sh
set -eu

base=/Users/Shared/herdr-deck-demo
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
output="$base/output/herdr-deck-showcase.mp4"
captions="$base/captions"
font='/System/Library/Fonts/Supplemental/Arial Bold.ttf'

mkdir -p "$captions"

caption() {
  name=$1
  size=$2
  copy=$3
  magick \
    -background '#000000C7' \
    -fill white \
    -font "$font" \
    -pointsize "$size" \
    -gravity center \
    "label:$copy" \
    -bordercolor '#000000C7' \
    -border 30x24 \
    "$captions/$name.png"
}

caption 01a 64 'Blocked agents rise to the top.'
caption 01b 64 'Browse by project, with the live layout in view.'
caption 02a 64 'Choose the same checkout or a worktree, then pick the agent.'
caption 02b 64 'Worktrunk resolves the checkout, then Herdr launches it.'
caption 03a 64 'Editor, agent, and shell in one workspace.'
caption 03b 64 'One cockpit keeps everything in view.'
caption 04 64 'Jump between your two most recent projects.'
caption 05a 64 'Resume saved conversations in project context.'
caption 05b 64 'Filter session history by agent.'
caption 06a 64 'Batch cleanup selects only clean, integrated worktrees.'
caption 06b 64 'Dirty work is identified and skipped.'
caption 07a 64 'Unmerged work stays protected behind an explicit force gate.'
caption 07b 64 'Create a new project directory without leaving the deck.'
caption 07c 72 'One picker. Your whole deck.'

ffmpeg -hide_banner -y \
  -i "$base/output/scene-01-browse.mp4" \
  -i "$base/output/scene-02-launch.mp4" \
  -i "$base/output/scene-03-deck.mp4" \
  -i "$base/output/scene-04-toggle.mp4" \
  -i "$base/output/scene-05-sessions.mp4" \
  -i "$base/output/scene-06-cleanup.mp4" \
  -i "$base/output/scene-07-safety.mp4" \
  -loop 1 -framerate 60 -i "$captions/01a.png" \
  -loop 1 -framerate 60 -i "$captions/01b.png" \
  -loop 1 -framerate 60 -i "$captions/02a.png" \
  -loop 1 -framerate 60 -i "$captions/02b.png" \
  -loop 1 -framerate 60 -i "$captions/03a.png" \
  -loop 1 -framerate 60 -i "$captions/03b.png" \
  -loop 1 -framerate 60 -i "$captions/04.png" \
  -loop 1 -framerate 60 -i "$captions/05a.png" \
  -loop 1 -framerate 60 -i "$captions/05b.png" \
  -loop 1 -framerate 60 -i "$captions/06a.png" \
  -loop 1 -framerate 60 -i "$captions/06b.png" \
  -loop 1 -framerate 60 -i "$captions/07a.png" \
  -loop 1 -framerate 60 -i "$captions/07b.png" \
  -loop 1 -framerate 60 -i "$captions/07c.png" \
  -filter_complex_script "$repo/artifacts/video-tools/showcase.filter" \
  -map '[outv]' \
  -an \
  -r 60 \
  -c:v h264_videotoolbox \
  -profile:v high \
  -level:v 5.2 \
  -b:v 40M \
  -maxrate 55M \
  -bufsize 80M \
  -pix_fmt yuv420p \
  -movflags +faststart \
  "$output"

cp "$output" "$repo/artifacts/herdr-deck-showcase.mp4"
