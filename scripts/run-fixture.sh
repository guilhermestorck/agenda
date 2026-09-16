#!/usr/bin/env bash
# Run agenda against the fixture calendar rather than the real one.
#
# Two things this exists to get right. The fixture lives outside the repo so it cannot be
# committed, and agenda is single-instance: launching a second copy hands the request to the
# running one and exits, so a copy already open on the real database will simply ignore the
# environment set here and show real data. Any running instance is therefore stopped first.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Under ~/.cache, not ~/.local/share: the fixture builder refuses to write anywhere beneath
# the real data directory, which is the right instinct — and this is regenerable anyway.
FIXTURE="${AGENDA_FIXTURE:-$HOME/.cache/agenda-fixture}"

if [ ! -f "$FIXTURE/fixture/agenda/agenda.db" ]; then
  echo "building the fixture at $FIXTURE"
  python3 "$ROOT/scripts/make-fixture.py" "$FIXTURE/fixture" >/dev/null
  mkdir -p "$FIXTURE/config/agenda"
  # A placeholder client: the fixture never reaches Google, and without one the window
  # opens on the "set up Google access" screen instead of the calendar.
  cat > "$FIXTURE/config/agenda/oauth.toml" <<'TOML'
client_id = "fixture-not-a-real-client.apps.googleusercontent.com"
client_secret = "fixture-placeholder-never-valid"
TOML
  chmod 600 "$FIXTURE/config/agenda/oauth.toml"
fi

if pgrep -f '[t]arget/release/agenda' >/dev/null || pgrep -x agenda >/dev/null; then
  echo "stopping the running instance first — otherwise it would just raise that window"
  pkill -f '[t]arget/release/agenda' 2>/dev/null || true
  pkill -x agenda 2>/dev/null || true
  sleep 1
fi

cargo build --release --manifest-path "$ROOT/Cargo.toml" 2>&1 | tail -1
echo "four accounts, all showing as disconnected — the fixture has no credentials"
exec env XDG_DATA_HOME="$FIXTURE/fixture" XDG_CONFIG_HOME="$FIXTURE/config" \
  "$ROOT/target/release/agenda" "$@"
