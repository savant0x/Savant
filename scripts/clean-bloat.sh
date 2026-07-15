#!/usr/bin/env bash
# scripts/clean-bloat.sh — transient + dead file cleanup (FID-024 §Step A.4 + LESSON-050)
#
# Per LESSON-029 + LESSON-050: remove .tmp-* + *.bak + dead-* + .scratch-* + .DS_Store files
# Per LESSON-050 step 5: track removals in `## Transient bloat removed` output block
# Per LESSON-052: dry-run + --apply pattern; idempotent-floor pattern (skip if 0 candidates)

set -euo pipefail

APPLY=false
if [ "${1:-}" = "--apply" ]; then
  APPLY=true
fi

# Find all transient/dead files, excluding build/cache dirs
CANDIDATES=$(find . \( \
    -path './node_modules' -o \
    -path './.git' -o \
    -path './target' -o \
    -path './.next' -o \
    -path './src-tauri/target' -o \
    -path './out' \
  \) -prune -o \
  \( -name '.tmp-*' -o -name '*.bak' -o -name 'dead-*' -o -name '.scratch-*' -o -name '.DS_Store' \) -print 2>/dev/null || true)

# Filter out empty results
COUNT=0
if [ -n "$CANDIDATES" ]; then
  COUNT=$(echo "$CANDIDATES" | wc -l | tr -d ' ')
fi

if [ "$COUNT" -eq 0 ]; then
  echo "[OK] No transient files found (discipline upheld)."
  exit 0
fi

MODE="[DRY-RUN]"
[ "$APPLY" = true ] && MODE="[APPLY]"
echo "$MODE Found $COUNT transient file(s):"
echo ""
echo "$CANDIDATES" | sed 's/^/  /'
echo ""

if [ "$APPLY" = false ]; then
  echo "[INFO] Run with --apply to actually remove:"
  echo "       bash scripts/clean-bloat.sh --apply"
  exit 0
fi

REMOVED=0
while IFS= read -r FILE; do
  if [ -n "$FILE" ]; then
    rm -f "$FILE"
    REMOVED=$((REMOVED + 1))
    echo "[REMOVED] $FILE"
  fi
done <<< "$CANDIDATES"

echo ""
echo "## Transient bloat removed"
echo ""
echo "  $REMOVED file(s) removed via scripts/clean-bloat.sh --apply:"
echo "$CANDIDATES" | sed 's/^/    - /'
echo ""
echo "[OK] Cleanup complete: $REMOVED file(s) removed."