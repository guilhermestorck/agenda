#!/usr/bin/env bash
# Screenshot the agenda window — and only the agenda window.
#
# `spectacle -a` captures whatever happens to be focused. Twice during development that was
# not agenda but the user's private Slack and a Zoom call with colleagues on camera. Neither
# should ever have reached a transcript. So this asks KWin what is actually focused first and
# refuses if the answer is anything but agenda.
#
# Usage: scripts/shot-agenda.sh <output.png>
set -euo pipefail

out="${1:?usage: shot-agenda.sh <output.png>}"
probe="$(mktemp --suffix=.js)"
trap 'rm -f "$probe"' EXIT

cat > "$probe" <<'JS'
const w = workspace.activeWindow;
print("SHOTGUARD|" + (w ? w.resourceClass : "none"));
JS

name="shotguard-$$"
gdbus call --session --dest org.kde.KWin --object-path /Scripting \
  --method org.kde.kwin.Scripting.loadScript "$probe" "$name" >/dev/null
gdbus call --session --dest org.kde.KWin --object-path /Scripting \
  --method org.kde.kwin.Scripting.start >/dev/null
sleep 0.4

active="$(journalctl --user -n 40 --since '10 seconds ago' 2>/dev/null \
  | grep -o 'SHOTGUARD|[A-Za-z0-9._-]*' | tail -1 | cut -d'|' -f2)"

if [ -z "$active" ]; then
  echo "REFUSED: could not determine the focused window" >&2
  exit 2
fi
if [ "$active" != "agenda" ]; then
  echo "REFUSED: focused window is '$active', not agenda" >&2
  exit 3
fi

spectacle -a -b -n -o "$out" >/dev/null 2>&1
sleep 1.5
[ -s "$out" ] || { echo "REFUSED: no image produced" >&2; exit 4; }
echo "captured $active -> $out"
