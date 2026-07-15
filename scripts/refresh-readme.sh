#!/usr/bin/env bash
# scripts/refresh-readme.sh — README refresh on release cut (FID-024 §Step A.3)
#
# Per `coding-standards/release-workflow.md` §Checkpoint Release Discipline:
#   - Update README status badge to v<X.Y.Z>_Released
#   - Replace "What's New in v<OLD>" with "What's New in v<NEW>" (per user's "no multiple What's New" rule)
#   - Update Roadmap table: current version SHIPPED + new version PLANNED
#   - Move CHANGELOG `## [Unreleased]` content → `## v<X.Y.Z>` + new empty `## [Unreleased]`
#
# Per LESSON-019: only run at release time (never speculatively)
# Per LESSON-061: pure awk (no python3 dependency — Git for Windows doesn't ship Python)

set -euo pipefail

TARGET=""
PREVIOUS_OVERRIDE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --previous)
            shift
            if [ $# -eq 0 ]; then
                echo "[FAIL] --previous requires a value" >&2
                exit 4
            fi
            PREVIOUS_OVERRIDE="$1"
            shift
            ;;
        -h|--help)
            echo "Usage: $0 <target> [--previous <ver>] (e.g., 0.0.7)" >&2
            exit 0
            ;;
        -*)
            echo "[FAIL] Unknown option: $1" >&2
            exit 4
            ;;
        *)
            if [ -z "$TARGET" ]; then
                TARGET="$1"
            fi
            shift
            ;;
    esac
done

if [ -z "$TARGET" ]; then
  echo "[FAIL] Usage: $0 <target> [--previous <ver>] (e.g., 0.0.7)" >&2
  exit 4
fi

if ! echo "$TARGET" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "[FAIL] Invalid version format: $TARGET" >&2
  exit 4
fi

# Resolve PREVIOUS: prefer explicit --previous override (used by orchestrator
# after a bump-version.sh call has already mutated VERSION; the orchestrator
# captured PREVIOUS at its own startup). Fall back to VERSION file for
# standalone callers.
if [ -n "$PREVIOUS_OVERRIDE" ]; then
  PREVIOUS="$PREVIOUS_OVERRIDE"
else
  PREVIOUS=$(cat VERSION)
fi

if [ "$PREVIOUS" = "$TARGET" ]; then
  echo "[FAIL] VERSION already at $TARGET. Run bump-version.sh first or pick a different version." >&2
  exit 2
fi

DATE=$(date +%Y-%m-%d)

# Compute next version (simple patch bump)
IFS='.' read -r MAJOR MINOR PATCH <<< "$TARGET"
NEXT_PATCH=$((PATCH + 1))
NEXT_VERSION="$MAJOR.$MINOR.$NEXT_PATCH"

echo "[refresh-readme.sh] Previous: $PREVIOUS → Target: $TARGET → Next: $NEXT_VERSION (date: $DATE)"
echo ""

# 1. README status badge
echo "[STEP 1] Updating README status badge..."
if grep -q "Status-v${PREVIOUS}_Released" README.md; then
  sed -i "s/Status-v${PREVIOUS}_Released/Status-v${TARGET}_Released/g" README.md
  echo "[OK] Status badge: v${PREVIOUS}_Released → v${TARGET}_Released"
else
  echo "[WARN] No badge match for v${PREVIOUS}_Released; badge may need manual update"
fi
echo ""

# 2. README "What's New" section — per user's "no multiple What's New" rule, REPLACE old with new
echo "[STEP 2] Replacing 'What's New in v${PREVIOUS}' with 'What's New in v${TARGET}'..."
if grep -q "## What's New in v${PREVIOUS}" README.md; then
  sed -i "s/## What's New in v${PREVIOUS}/## What's New in v${TARGET}/g" README.md
  echo "[OK] 'What's New' header updated: v${PREVIOUS} → v${TARGET}"
else
  echo "[WARN] No 'What's New in v${PREVIOUS}' section found; may already be updated"
fi
echo ""

# 3. README Roadmap table — flip v<TARGET> row to SHIPPED + add v<NEXT_VERSION> PLANNED row
echo "[STEP 3] Updating Roadmap table..."
# Find the v<TARGET> row and flip its status from PLANNED/IN PROGRESS → SHIPPED
if grep -E "^\| v${TARGET//./\\.} " README.md > /dev/null; then
  # v<TARGET> row exists; flip its status
  sed -i -E "s/^(\| v${TARGET//./\\.} \|[^|]+\|)[^|]+(\|)/\1 SHIPPED \2/" README.md
  echo "[OK] Roadmap v${TARGET} row flipped to SHIPPED"
else
  echo "[INFO] No existing Roadmap row for v${TARGET}; skipping status flip"
fi

# Add new v<NEXT_VERSION> row if not present
if ! grep -E "^\| v${NEXT_VERSION//./\\.} " README.md > /dev/null; then
  # Find the last Roadmap table row and append after it
  # Pattern: | v<X.Y.Z>  |   <phase>  | <status>  | <focus> |
  LAST_ROW=$(grep -nE '^\| v[0-9]+\.[0-9]+\.[0-9]+ ' README.md | tail -n 1 | cut -d: -f1)
  if [ -n "$LAST_ROW" ]; then
    # Insert a new row after the last row
    sed -i "${LAST_ROW}a\\| v${NEXT_VERSION}  |   1+  | PLANNED  | (TBD: scope for v${NEXT_VERSION} — open candidates: FID-029 §Step 2-5 / FID-030 / FID-032 / FID-033 / FID-034 / FID-035) |" README.md
    echo "[OK] Roadmap v${NEXT_VERSION} PLANNED row added"
  else
    echo "[WARN] Could not find Roadmap table to append new new row"
  fi
else
  echo "[INFO] Roadmap v${NEXT_VERSION} row already exists; skipping add"
fi
echo ""

# 4. CHANGELOG — promote ## [Unreleased] → ## v<TARGET> + add new empty ## [Unreleased] for v<NEXT_VERSION>
# Per LESSON-061: pure awk (no python3 dependency — Git for Windows doesn't ship Python).
# Two-pass awk: rename FIRST ## [Unreleased] → new version header, then insert a new
# ## [Unreleased] block immediately after the # Changelog H1 header.
echo "[STEP 4] Promoting CHANGELOG ## [Unreleased] → ## v${TARGET} — ${DATE}..."

# 4a. Promote the FIRST "## [Unreleased]" header to "## v<TARGET> — <DATE>" via awk (POSIX, ships in Git for Windows).
# awk hex escape \xE2\x80\x94 = UTF-8 em-dash (3 bytes); bash single-quoted string passes it through literally.
awk -v target="$TARGET" -v date="$DATE" '
/^## \[Unreleased\]/ && !found {
    print "## v" target " \xE2\x80\x94 " date
    found = 1
    next
}
{ print }
' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md
echo "  [OK] Promoted first '[Unreleased]' header to '## v${TARGET} — ${DATE}'"

# 4b. Insert a NEW empty ## [Unreleased] section immediately after the '# Changelog' H1 header.
# Idempotent via the `!inserted` flag — only fires on the first H1 match.
awk -v next_ver="$NEXT_VERSION" '
/^# Changelog/ && !inserted {
    print $0
    print ""
    print "## [Unreleased]"
    print ""
    print "Work-in-progress against v" next_ver ". Open candidates: (a) FID-029 §Step 2-5 (chat persistence renderer-side); (b) FID-030 (CLI scaffold); (c) FID-032 (api-client refactor); (d) FID-033 (Tauri repackaging to apps/tauri/); (e) FID-034 (kernel trait adoption); (f) FID-035 master-FID §Layered Build Order. Awaiting begin-ratification per LESSON-051."
    print ""
    inserted = 1
    next
}
{ print }
' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md
echo "  [OK] Added new empty '[Unreleased]' section"

echo "[OK] CHANGELOG.md promoted."
echo ""
echo "[OK] README + CHANGELOG refresh complete for v$TARGET."
echo "  Status badge: v${TARGET}_Released"
echo "  What's New: v${PREVIOUS} → v${TARGET}"
echo "  Roadmap v${TARGET}: SHIPPED"
echo "  Roadmap v${NEXT_VERSION}: PLANNED"
echo "  CHANGELOG: ## [Unreleased] promoted to ## v${TARGET} — ${DATE}; new ## [Unreleased] added"
