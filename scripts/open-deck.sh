#!/usr/bin/env bash
set -euo pipefail

plugin_root=${HERDR_PLUGIN_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)}
herdr=${HERDR_BIN_PATH:-herdr}
deck="$plugin_root/target/release/herdr-deck"

cwd=$("$deck" --print-plugin-cwd)
[[ -n $cwd ]] || cwd=$PWD

exec "$herdr" plugin pane open \
  --plugin "${HERDR_PLUGIN_ID:-herdr-deck}" \
  --entrypoint picker \
  --placement split \
  --direction down \
  --cwd "$cwd" \
  --focus
