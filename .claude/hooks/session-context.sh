#!/usr/bin/env bash
# SessionStart: inject current git state so the session starts oriented.
set -uo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"; . "$DIR/_lib.sh"
cat >/dev/null 2>&1 || true   # drain (payload unused)

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || true
command -v git >/dev/null 2>&1 || exit 0
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
dirty=$(git status --short 2>/dev/null | wc -l | tr -d ' ')
last=$(git log -1 --oneline 2>/dev/null)

emit_context "SessionStart" "Git: on '${branch}', ${dirty} uncommitted file(s). Last commit: ${last}"
exit 0
