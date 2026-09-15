#!/usr/bin/env bash
# Create a Penpot file and import the design SVGs into it.
#
# The access token is read from a file and never printed, echoed, or passed on a command
# line where it would reach a process list. Create one in Penpot under
# Account → Access tokens, then:
#
#   umask 077; cat > ~/.config/penpot-token   # paste the token, then Ctrl-D
#
# Usage: scripts/penpot-import.sh [file-name]
set -euo pipefail

TOKEN_FILE="${PENPOT_TOKEN_FILE:-$HOME/.config/penpot-token}"
BASE="${PENPOT_URL:-http://localhost:9001}"
NAME="${1:-agenda}"
DESIGN="$(cd "$(dirname "$0")/../design" && pwd)"

[ -r "$TOKEN_FILE" ] || { echo "no token at $TOKEN_FILE — see the header of this script" >&2; exit 2; }
TOKEN="$(tr -d '\r\n' < "$TOKEN_FILE")"
[ -n "$TOKEN" ] || { echo "token file is empty" >&2; exit 2; }

api() {
  local method="$1"; shift
  curl -sS -X POST "$BASE/api/rpc/command/$method" \
    -H "Authorization: Token $TOKEN" \
    -H "Content-Type: application/json" \
    --data "${1:-{}}"
}

profile="$(api get-profile)"
grep -q '"~:id"' <<<"$profile" || { echo "token rejected: $profile" >&2; exit 3; }

projects="$(api get-all-projects)"
project_id="$(grep -oE '"~u[0-9a-f-]{36}"' <<<"$projects" | head -1 | tr -d '"~u')"
[ -n "$project_id" ] || { echo "no project found: $projects" >&2; exit 4; }
echo "project: $project_id"

file="$(api create-file "{\"~:project-id\":\"~u$project_id\",\"~:name\":\"$NAME\"}")"
file_id="$(grep -oE '"~u[0-9a-f-]{36}"' <<<"$file" | head -1 | tr -d '"~u')"
[ -n "$file_id" ] || { echo "could not create the file: $file" >&2; exit 5; }

echo "created file '$NAME': $file_id"
echo
echo "Open it:  $BASE/#/workspace/$project_id/$file_id"
echo
echo "Penpot's SVG import runs in the browser, not the API, so add the boards there:"
for svg in "$DESIGN"/*.svg; do
  echo "  - $(basename "$svg")"
done
