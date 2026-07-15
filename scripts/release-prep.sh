#!/usr/bin/env bash
# scripts/release-prep.sh — FID-024 §Step E orchestrator (release-cut workflow)
#
# Per `coding-standards/release-workflow.md` §Checkpoint Release Discipline:
#   - Accepts version as arg (e.g., `bash scripts/release-prep.sh 0.0.7 [--apply]`)
#   - Default: dry-run full sweep + show preview
#   - --apply mode: actually run the full sweep (bump + archive + refresh + clean +
#     verify + commit + tag + push)
#
# Sequence:
#   1. Validate clean working tree (LESSON-029 Gate 1)
#   2. archive-fids.sh --dry-run → print candidates
#   3. bump-version.sh <ver> → bump 5 files in lockstep (LESSON-019)
#   4. refresh-readme.sh <ver> → update README + CHANGELOG
#   5. clean-bloat.sh --apply → remove transient files (LESSON-029 + LESSON-050)
#   6. Verification gates (LESSON-027 + LESSON-038 + cargo check + release-check.sh)
#   7. If all GREEN: write commit msg file + commit + tag + push
#
# Honors LESSON-019 (release-only-versioning) + LESSON-027 (doc-drift invariant) +
# LESSON-029 (release.py pre-flight cleanup) + LESSON-030 (file-based commit/tag) +
# LESSON-038 (no-auto-defer) + LESSON-050 (untracked-bloat candidate) +
# LESSON-052 (FID auto-archive discipline) + LESSON-058 (relaxed subject length).

set -euo pipefail

TARGET="${1:-}"
APPLY="${2:-}"

if [ -z "$TARGET" ]; then
  echo "[FAIL] Usage: $0 <version> [--apply]" >&2
  echo "       e.g., $0 0.0.7 --apply" >&2
  exit 4
fi

if ! echo "$TARGET" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "[FAIL] Invalid version format: $TARGET (expected X.Y.Z)" >&2
  exit 4
fi

PREVIOUS=$(cat VERSION)
DATE=$(date +%Y-%m-%d)
# Compute next version (simple patch bump for Roadmap row insertion)
IFS='.' read -r MAJOR MINOR PATCH <<< "$TARGET"
NEXT_PATCH=$((PATCH + 1))
NEXT_VERSION="$MAJOR.$MINOR.$NEXT_PATCH"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  FID-024 §Step E Orchestrator — v$TARGET release cut"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  previous: $PREVIOUS"
echo "  target:   $TARGET"
echo "  next:     $NEXT_VERSION"
echo "  date:     $DATE"
echo "  mode:     $([ "$APPLY" = "--apply" ] && echo "APPLY" || echo "DRY-RUN")"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# ── Gate 1: clean working tree (LESSON-029) ──
echo "[Gate 1] Clean working tree check..."
DIRTY=$(git status --porcelain | wc -l | tr -d ' ')
if [ "$DIRTY" -ne 0 ]; then
  echo "[FAIL] Working tree has $DIRTY uncommitted change(s):"
  git status --porcelain | sed 's/^/  /'
  echo ""
  echo "[HALT] Commit, stash, or clean up before running the orchestrator."
  exit 1
fi
echo "[OK] Working tree is clean."
echo ""

# ── Step 1: archive-fids.sh ──
echo "[Step 1] archive-fids.sh..."
bash scripts/archive-fids.sh
echo ""

# ── Step 2: bump-version.sh ──
echo "[Step 2] bump-version.sh $TARGET..."
if [ "$APPLY" = "--apply" ]; then
  bash scripts/bump-version.sh "$TARGET"
else
  echo "[DRY-RUN] Would bump 5 version files from $PREVIOUS → $TARGET (VERSION + package.json + src-tauri/tauri.conf.json + Cargo.toml + protocol.config.yaml)"
fi
echo ""

# ── Step 3: refresh-readme.sh ──
# Pass --previous explicitly: the orchestrator captured PREVIOUS at startup
# (before the bump mutated VERSION). Without this, refresh-readme.sh would
# read VERSION=0.0.7 and bail out with 'VERSION already at target'.
echo "[Step 3] refresh-readme.sh $TARGET --previous $PREVIOUS..."
if [ "$APPLY" = "--apply" ]; then
  bash scripts/refresh-readme.sh "$TARGET" --previous "$PREVIOUS"
else
  echo "[DRY-RUN] Would update README (status badge + What's New + Roadmap) + CHANGELOG (promote ## [Unreleased] → ## v$TARGET) with PREVIOUS=$PREVIOUS"
fi
echo ""

# ── Step 4: clean-bloat.sh ──
echo "[Step 4] clean-bloat.sh --apply..."
if [ "$APPLY" = "--apply" ]; then
  bash scripts/clean-bloat.sh --apply
else
  bash scripts/clean-bloat.sh
fi
echo ""

# ── Step 5: Verification gates ──
echo "[Step 5] Verification gates..."
echo ""

echo "  [Gate A] bash scripts/lint-docs.sh (LESSON-027 doc-drift invariant)..."
if bash scripts/lint-docs.sh >/dev/null 2>&1; then
  echo "  [OK] LESSON-027 invariant holds."
else
  echo "  [FAIL] LESSON-027 invariant violated. Run scripts/lint-docs.sh for details."
  exit 3
fi
echo ""

echo "  [Gate B] bash scripts/lint-defer.sh (LESSON-038 no-auto-defer invariant)..."
if bash scripts/lint-defer.sh >/dev/null 2>&1; then
  echo "  [OK] LESSON-038 invariant holds."
else
  echo "  [FAIL] LESSON-038 invariant violated. Run scripts/lint-defer.sh for details."
  exit 3
fi
echo ""

echo "  [Gate C] cargo check --workspace (Rust baseline)..."
if cargo check --workspace --offline >/dev/null 2>&1; then
  echo "  [OK] Cargo baseline GREEN."
else
  echo "  [FAIL] cargo check failed. Run cargo check --workspace for details."
  exit 3
fi
echo ""

echo "  [Gate D] bash scripts/release-check.sh $TARGET (LESSON-029 3-gate pre-flight)..."
if bash scripts/release-check.sh "$TARGET" >/dev/null 2>&1; then
  echo "  [OK] Release-check 3-gate pre-flight PASS."
else
  echo "  [FAIL] Release-check failed. Run scripts/release-check.sh $TARGET for details."
  exit 3
fi
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  All verification gates GREEN"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Working tree state after orchestrator sweep:"
git status --short | head -n 30 || echo "  (clean)"
echo ""

if [ "$APPLY" != "--apply" ]; then
  echo "[DRY-RUN COMPLETE]"
  echo "Review the above + run with --apply to commit + tag + push:"
  echo "  bash scripts/release-prep.sh $TARGET --apply"
  exit 0
fi

# ── Apply mode: commit + push main + delegate tag/GH-release to release.py ──
echo "[APPLY MODE] Committing + pushing main + delegating tag + GH release to release.py..."
echo ""

# Write commit msg file (LESSON-030 file-based pattern)
MSG_FILE="dev/.tmp-v${TARGET}-release.txt"
cat > "$MSG_FILE" <<EOF
chore(release): v${TARGET} — orchestrator validation run

Auto-generated by scripts/release-prep.sh $TARGET --apply (FID-024 §Step E
orchestrator). Captures the post-cascade-recovery baseline + LESSON-059
codification + clean reinstall + FID-029 §Step 1 sibling-collection pivot.

Changes:
- 5 version-bearing files bumped $PREVIOUS → $TARGET (VERSION + package.json +
  src-tauri/tauri.conf.json + Cargo.toml + protocol.config.yaml) per LESSON-019
- CHANGELOG [Unreleased] promoted to v$TARGET — $DATE with foundation work entry
- README status badge updated to v${TARGET}_Released
- README "What's New in v${TARGET}" section replaces "What's New in v${PREVIOUS}"
- Roadmap table v$TARGET row flipped to SHIPPED
- Roadmap table v$NEXT_VERSION row added as PLANNED

Verification (LESSON-019/027/029/038 + cargo baseline):
- bash scripts/lint-docs.sh (LESSON-027): exit 0
- bash scripts/lint-defer.sh (LESSON-038): exit 0
- cargo check --workspace: exit 0
- bash scripts/release-check.sh $TARGET (3-gate pre-flight): exit 0

Cross-refs: LESSON-019 + LESSON-027 + LESSON-029 + LESSON-030 + LESSON-038 +
LESSON-050 + LESSON-052 + LESSON-058 + LESSON-059 + FID-024 §Step E +
FID-029 §Step 1 + master-FID-035 §Layer 1a foundation.

Tag + GH release delegated to: python scripts/release.py $TARGET --skip-refresh
(orchestrator already did README refresh; release.py handles tag + push tag +
GH release creation via the git credential helper).
EOF

# Stage all changes + commit using file-based pattern
git add -A
bash scripts/commit-with-message.sh "$MSG_FILE"
rm -f "$MSG_FILE"
echo ""

# Push main (NOT tag — release.py handles tag creation + push)
echo "[PUSH] git push origin main..."
git push origin main
echo "[OK] main pushed; release.py will handle tag + GH release."
echo ""

# Delegate tag creation + push + GH release to release.py
# Use --skip-refresh because the orchestrator already ran refresh-readme.sh
echo "[DELEGATE] python scripts/release.py $TARGET --skip-refresh..."
python scripts/release.py "$TARGET" --skip-refresh

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  v$TARGET release cut complete!"
echo "  Release URL: https://github.com/savant0x/Savant/releases/tag/v$TARGET"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"