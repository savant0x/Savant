#!/usr/bin/env bash
# scripts/bump-version.sh — lockstep version bump for 5 files (FID-024 §Step A.2)
#
# Per `coding-standards/release-workflow.md` §Checkpoint Release Discipline:
#   - Accepts version as arg (e.g., `bash scripts/bump-version.sh 0.0.7`)
#   - Validates all 5 version-bearing files currently at same version
#   - Updates all 5 to target version in lockstep per LESSON-019
#
# Per LESSON-019: versions rock ONLY at release time; never bump speculatively

set -euo pipefail

TARGET="${1:-}"
if [ -z "$TARGET" ]; then
  echo "[FAIL] Usage: $0 <version> (e.g., 0.0.7)" >&2
  exit 4
fi

# Validate target format (X.Y.Z)
if ! echo "$TARGET" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "[FAIL] Invalid version format: $TARGET (expected X.Y.Z)" >&2
  exit 4
fi

echo "[bump-version.sh] Target: $TARGET"
echo ""

# Read current version from each file
get_version() {
  local FILE="$1"
  case "$FILE" in
    VERSION)
      grep -oE '[0-9]+\.[0-9]+\.[0-9]+' "$FILE" | head -n 1
      ;;
    package.json)
      grep -oE '"version":\s*"[0-9]+\.[0-9]+\.[0-9]+"' "$FILE" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1
      ;;
    src-tauri/tauri.conf.json)
      grep -oE '"version":\s*"[0-9]+\.[0-9]+\.[0-9]+"' "$FILE" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1
      ;;
    Cargo.toml)
      grep -E '^version\s*=\s*"[0-9]+\.[0-9]+\.[0-9]+"' "$FILE" | head -n 1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+'
      ;;
    protocol.config.yaml)
      grep -E '^\s*version:\s*"[0-9]+\.[0-9]+\.[0-9]+"' "$FILE" | head -n 1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+'
      ;;
  esac
}

FILES=(
  "VERSION"
  "package.json"
  "src-tauri/tauri.conf.json"
  "Cargo.toml"
  "protocol.config.yaml"
)

# Verify all files exist
for FILE in "${FILES[@]}"; do
  if [ ! -f "$FILE" ]; then
    echo "[FAIL] File not found: $FILE" >&2
    exit 1
  fi
done

# Read all versions
echo "[PREFLIGHT] Current versions:"
VERSIONS=()
for FILE in "${FILES[@]}"; do
  V=$(get_version "$FILE")
  if [ -z "$V" ]; then
    echo "  $FILE: [NOT FOUND]"
    echo "[FAIL] Could not extract version from $FILE" >&2
    exit 1
  fi
  VERSIONS+=("$V")
  echo "  $FILE: $V"
done
echo ""

# Validate all 5 at same version
UNIQUE_COUNT=$(printf '%s\n' "${VERSIONS[@]}" | sort -u | wc -l | tr -d ' ')
if [ "$UNIQUE_COUNT" -ne 1 ]; then
  echo "[FAIL] Version drift detected across files:" >&2
  for i in "${!FILES[@]}"; do
    echo "  ${FILES[$i]}: ${VERSIONS[$i]}" >&2
  done
  echo "[FAIL] All 5 files must be at the same version before bump." >&2
  exit 2
fi

CURRENT="${VERSIONS[0]}"
echo "[PREFLIGHT] All 5 files at version $CURRENT (lockstep OK)"
echo ""

if [ "$CURRENT" = "$TARGET" ]; then
  echo "[OK] Files already at target version $TARGET; no changes needed."
  exit 0
fi

echo "[BUMP] $CURRENT → $TARGET"
echo ""

# Update VERSION (single-line file)
echo "$TARGET" > VERSION
echo "[UPDATED] VERSION"

# Update package.json
sed -i "s/\"version\": \"$CURRENT\"/\"version\": \"$TARGET\"/" package.json
echo "[UPDATED] package.json"

# Update src-tauri/tauri.conf.json
sed -i "s/\"version\": \"$CURRENT\"/\"version\": \"$TARGET\"/" src-tauri/tauri.conf.json
echo "[UPDATED] src-tauri/tauri.conf.json"

# Update Cargo.toml (workspace.package.version)
sed -i "s/^version = \"$CURRENT\"/version = \"$TARGET\"/" Cargo.toml
echo "[UPDATED] Cargo.toml"

# Update protocol.config.yaml (project.version)
sed -i "s/^\(\s*version:\s*\)\"$CURRENT\"/\1\"$TARGET\"/" protocol.config.yaml
echo "[UPDATED] protocol.config.yaml"

echo ""
echo "[VERIFY] Post-bump versions:"
for FILE in "${FILES[@]}"; do
  echo "  $FILE: $(get_version "$FILE")"
done

echo ""
echo "[OK] All 5 version-bearing files bumped: $CURRENT → $TARGET"