#!/usr/bin/env bash
set -Eeuo pipefail
IFS=$'\n\t'

REPO_URL="https://github.com/LibreOffice/dictionaries.git"
BRANCH="master"

BASE_DIR="/opt/spellbook-dictionaries"
SRC_DIR="${BASE_DIR}/src"
RELEASES_DIR="${BASE_DIR}/releases"
CURRENT_LINK="${BASE_DIR}/current"
LOG_DIR="${BASE_DIR}/logs"
LOCK_FILE="${BASE_DIR}/update.lock"

# Space-separated allowlist. Adjust to your app's needs.
LOCALES="${LOCALES:-en_US en_GB es_ES fr_FR de_DE it_IT pt_BR pt_PT nl_NL}"
KEEP_RELEASES="${KEEP_RELEASES:-10}"

GIT="${GIT:-/usr/bin/git}"
DATE="${DATE:-/usr/bin/date}"
FLOCK="${FLOCK:-/usr/bin/flock}"
FIND="${FIND:-/usr/bin/find}"
CP="${CP:-/usr/bin/cp}"
LN="${LN:-/usr/bin/ln}"
MKDIR="${MKDIR:-/usr/bin/mkdir}"
RM="${RM:-/usr/bin/rm}"
SORT="${SORT:-/usr/bin/sort}"
TAIL="${TAIL:-/usr/bin/tail}"
SHA256SUM="${SHA256SUM:-/usr/bin/sha256sum}"

umask 027
mkdir -p "$SRC_DIR" "$RELEASES_DIR" "$LOG_DIR"

LOGFILE="${LOG_DIR}/update-$($DATE +%F).log"
exec >>"$LOGFILE" 2>&1

echo "=================================================="
echo "[$($DATE --iso-8601=seconds)] spellbook dictionary update started"

cleanup() {
  if [[ -n "${TMPDIR:-}" && -d "${TMPDIR:-}" ]]; then
    rm -rf "$TMPDIR"
  fi
}
trap cleanup EXIT

exec 9>"$LOCK_FILE"
if ! "$FLOCK" -n 9; then
  echo "Another update is already running; exiting."
  exit 0
fi

ensure_clone() {
  if [[ ! -d "$SRC_DIR/.git" ]]; then
    echo "Cloning upstream repository..."
    "$GIT" clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$SRC_DIR"
  fi
}

sync_repo() {
  cd "$SRC_DIR"
  OLD_SHA="$("$GIT" rev-parse HEAD)"

  echo "Fetching upstream..."
  "$GIT" fetch --depth 1 origin "$BRANCH"
  "$GIT" reset --hard "origin/$BRANCH"
  "$GIT" clean -xfd

  NEW_SHA="$("$GIT" rev-parse HEAD)"

  if [[ "$OLD_SHA" == "$NEW_SHA" ]]; then
    echo "No upstream changes; current commit is still $NEW_SHA."
    exit 0
  fi

  echo "Upstream changed: $OLD_SHA -> $NEW_SHA"
}

copy_locale_tree() {
  local locale="$1"
  local matches=()

  while IFS= read -r -d '' f; do
    matches+=("$f")
  done < <(
    "$FIND" . -type f \( \
      -path "./${locale}/*.dic" -o \
      -path "./${locale}/*.aff" -o \
      -path "./${locale}/*.txt" -o \
      -path "./${locale}/*.json" \
    \) -print0
  )

  if [[ "${#matches[@]}" -eq 0 ]]; then
    echo "WARN: no files found for locale $locale"
    return 0
  fi

  for src in "${matches[@]}"; do
    rel="${src#./}"
    dest="$TMPDIR/$rel"
    mkdir -p "$(dirname "$dest")"
    "$CP" -a "$src" "$dest"
  done
}

build_release() {
  TMPDIR="$(mktemp -d "${BASE_DIR}/.tmp.XXXXXX")"
  RELEASE_DIR="${RELEASES_DIR}/${NEW_SHA}"
  mkdir -p "$TMPDIR"

  echo "Building release in $TMPDIR"

  for locale in $LOCALES; do
    copy_locale_tree "$locale"
  done

  # Basic validation: each locale should have at least one .dic or .aff.
  for locale in $LOCALES; do
    if ! "$FIND" "$TMPDIR/$locale" -type f \( -name '*.dic' -o -name '*.aff' \) -print -quit | grep -q .; then
      echo "ERROR: validation failed for locale $locale"
      exit 1
    fi
  done

  cat > "$TMPDIR/manifest.json" <<EOF
{
  "source_repo": "LibreOffice/dictionaries",
  "source_branch": "$BRANCH",
  "source_commit": "$NEW_SHA",
  "generated_at": "$($DATE --iso-8601=seconds)",
  "locales": [$(printf '"%s",' $LOCALES | sed 's/,$//')]
}
EOF

  (
    cd "$TMPDIR"
    "$FIND" . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 "$SHA256SUM" > SHA256SUMS
  )

  mkdir -p "$(dirname "$RELEASE_DIR")"
  mv "$TMPDIR" "$RELEASE_DIR"
  TMPDIR=""

  "$LN" -sfn "$RELEASE_DIR" "$CURRENT_LINK"
  echo "$NEW_SHA" > "${BASE_DIR}/CURRENT"

  echo "Published release: $RELEASE_DIR"
}

prune_old_releases() {
  echo "Pruning old releases (keeping $KEEP_RELEASES)..."
  mapfile -t releases < <("$FIND" "$RELEASES_DIR" -mindepth 1 -maxdepth 1 -type d | "$SORT")
  count="${#releases[@]}"

  if (( count <= KEEP_RELEASES )); then
    echo "Nothing to prune."
    return 0
  fi

  to_delete=$((count - KEEP_RELEASES))
  # shellcheck disable=SC2066
  for old in "$("${TAIL}" -n "$to_delete" < <(printf '%s\n' "${releases[@]}"))"; do
    echo "Removing $old"
    "$RM" -rf "$old"
  done
}

main() {
  ensure_clone
  sync_repo
  build_release
  prune_old_releases
  echo "[$($DATE --iso-8601=seconds)] update complete"
}

main "$@"