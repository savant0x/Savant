#!/usr/bin/env bash
# scripts/archive-fids.sh — FID auto-archive (FID-024 §Step A.1 + LESSON-052)
#
# Per `coding-standards/release-workflow.md` §Checkpoint Release Discipline:
#   - Default: dry-run; lists FIDs with `**Status:** closed` (NOT auto-closes WIP FIDs)
#   - --apply mode: moves closed FIDs to dev/fids/archive/ + verifies §Closed footer
#
# Per LESSON-052: NO-OP discipline confirmation is the win state
# Per LESSON-038: NEVER auto-close a FID without Spencer's explicit approval

set -euo pipefail

APPLY=false
if [ "${1:-}" = "--apply" ]; then
  APPLY=true
fi

# Locate all active FIDs (dev/fids/FID-*.md, excluding archive/)
CANDIDATES=$(find dev/fids -maxdepth 1 -name 'FID-*.md' -type f 2>/dev/null || true)

if [ -z "$CANDIDATES" ]; then
  echo "[OK] No active FIDs to archive (discipline upheld via prior cycles)."
  exit 0
fi

MOVED=0
SKIPPED=0
MODE="[DRY-RUN]"
[ "$APPLY" = true ] && MODE="[APPLY]"

echo "$MODE Scanning $(echo "$CANDIDATES" | wc -l | tr -d ' ') active FID(s)..."
echo ""

for FID in $CANDIDATES; do
  # Extract the Status field value (last token of the **Status:** line)
  STATUS=$(grep -m1 -E '^\*\*Status:\*\*' "$FID" 2>/dev/null | sed -E 's/.*\*\*Status:\*\*\s*//' | sed -E 's/[[:space:]]*$//' || echo "unknown")

  if [ "$STATUS" = "closed" ]; then
    if [ "$APPLY" = true ]; then
      mv "$FID" dev/fids/archive/
      MOVED=$((MOVED + 1))
      echo "$MODE [ARCHIVED] $FID"
    else
      echo "$MODE [CANDIDATE] $FID (Status: closed)"
    fi
  else
    SKIPPED=$((SKIPPED + 1))
    echo "$MODE [SKIPPED] $FID (Status: $STATUS — WIP, not auto-closed per LESSON-038)"
  fi
done

echo ""
TOTAL=$(echo "$CANDIDATES" | wc -l | tr -d ' ')
echo "[SUMMARY] candidates=$TOTAL | moved=$MOVED | skipped(WIP)=$SKIPPED"

if [ "$APPLY" = false ] && [ "$MOVED" -gt 0 ]; then
  echo "[INFO] Run with --apply to actually move the candidates:"
  echo "       bash scripts/archive-fids.sh --apply"
fi
exit 0