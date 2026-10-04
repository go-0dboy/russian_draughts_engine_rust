#!/usr/bin/env bash
# Safe in-place updater for russian_draughts_classic.
# Can run either from inside the project as ./update.sh, or externally from
# Downloads while the current directory is the project root (bootstrap mode).

set -Eeuo pipefail

PROJECT_NAME="russian_draughts_classic"
ARCHIVE_PREFIX="russian_draughts_classic_v"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
DEFAULT_DOWNLOAD_DIR="${HOME}/storage/downloads"

package_field() {
    local cargo_toml="$1" field="$2"
    awk -v wanted="$field" '
        /^\[package\][[:space:]]*$/ { in_package=1; next }
        /^\[/ { if (in_package) exit }
        in_package {
            split($0, parts, "=")
            key=parts[1]
            gsub(/[[:space:]]/, "", key)
            if (key == wanted) {
                line=$0
                sub(/^[^=]*=[[:space:]]*/, "", line)
                gsub(/^[[:space:]"]+|[[:space:]"]+$/, "", line)
                print line
                exit
            }
        }
    ' "$cargo_toml"
}

package_name() { package_field "$1" name; }
package_version() { package_field "$1" version; }

is_project_root() {
    local dir="$1"
    [[ -f "$dir/Cargo.toml" ]] || return 1
    [[ "$(package_name "$dir/Cargo.toml" 2>/dev/null || true)" == "$PROJECT_NAME" ]]
}

# Normal mode: updater already lives in the project.
# Bootstrap mode: updater is in Downloads, but it is invoked while cwd is the project.
if is_project_root "$SCRIPT_DIR"; then
    ROOT="$SCRIPT_DIR"
elif is_project_root "$PWD"; then
    ROOT="$(pwd -P)"
else
    printf 'update error: cannot find %s project root\n' "$PROJECT_NAME" >&2
    printf 'run this script from the classic engine directory, or install it there as ./update.sh\n' >&2
    exit 1
fi

UPDATES_DIR="$ROOT/.updates"

usage() {
    cat <<USAGE
Usage:
  ./update.sh [release.zip]       Update to a release archive.
  ./update.sh --dry-run [zip]     Validate and show what would be updated.
  ./update.sh --status            Show current version and saved backups.
  ./update.sh --rollback          Roll back to the most recent source backup.
  ./update.sh --force [zip]       Allow same-version update or downgrade.

Bootstrap from Downloads (for v1.0.0, which had no updater):
  cd ~/projects/russian_draughts_classic
  bash ~/storage/downloads/update_classic.sh \\
    ~/storage/downloads/russian_draughts_classic_v1.5.1.zip

If release.zip is omitted, the newest matching
~/storage/downloads/${ARCHIVE_PREFIX}*.zip is selected.

Preserved across updates:
  target/ .git/ .updates/ books/ tablebases/ data/ logs/ and unknown user files.
  Release-owned tool/config files are listed explicitly; put generated corpora and tuned parameter files under data/.

Only paths listed in .release-manifest are replaced by a release.
Every real update creates a rollback backup under .updates/.
USAGE
}

die() {
    printf 'update error: %s\n' "$*" >&2
    exit 1
}

current_version() {
    package_version "$ROOT/Cargo.toml"
}

latest_archive() {
    local newest=""
    local files=()
    shopt -s nullglob
    files=("$DEFAULT_DOWNLOAD_DIR"/${ARCHIVE_PREFIX}*.zip)
    shopt -u nullglob
    ((${#files[@]} != 0)) || return 1
    newest="$(ls -1t -- "${files[@]}" 2>/dev/null | head -n 1)"
    [[ -n "$newest" ]] || return 1
    printf '%s\n' "$newest"
}

manifest_paths() {
    local manifest="$1"
    if [[ -f "$manifest" ]]; then
        sed -e 's/[[:space:]]*#.*$//' -e '/^[[:space:]]*$/d' "$manifest"
    else
        # Legacy v1.0.0 fallback: exact release-owned paths that existed before
        # .release-manifest was introduced.
        cat <<'FALLBACK'
Cargo.toml
src
tests
examples
README.md
ARCHITECTURE.md
TESTING.md
CHANGELOG.md
CODE_REVIEW.md
LICENSE
.gitignore
SOURCE_SHA256SUMS.txt
update.sh
FALLBACK
    fi
}

validate_manifest_path() {
    local p="$1"
    [[ -n "$p" && "$p" != "." && "$p" != "./" ]] || die "empty/root path in release manifest"
    [[ "$p" != /* ]] || die "absolute path is forbidden in release manifest: $p"
    [[ "$p" != *".."* ]] || die "parent traversal is forbidden in release manifest: $p"
    case "$p" in
        target|target/*|.git|.git/*|.updates|.updates/*|books|books/*|tablebases|tablebases/*|data|data/*|logs|logs/*)
            die "release manifest attempts to own preserved user/runtime path: $p"
            ;;
    esac
}

find_release_root() {
    local unpacked="$1"
    local cargo=""
    while IFS= read -r candidate; do
        if [[ "$(package_name "$candidate" 2>/dev/null || true)" == "$PROJECT_NAME" ]]; then
            cargo="$candidate"
            break
        fi
    done < <(find "$unpacked" -mindepth 1 -maxdepth 4 -type f -name Cargo.toml -print)
    [[ -n "$cargo" ]] || return 1
    dirname -- "$cargo"
}

version_is_older() {
    local candidate="$1" current="$2"
    [[ "$candidate" == "$current" ]] && return 1
    local first
    first="$(printf '%s\n%s\n' "$candidate" "$current" | sort -V | head -n 1)"
    [[ "$first" == "$candidate" ]]
}

show_status() {
    printf '%s v%s\n' "$PROJECT_NAME" "$(current_version)"
    printf 'directory : %s\n' "$ROOT"
    printf 'backups   :\n'
    if [[ -d "$UPDATES_DIR" ]]; then
        find "$UPDATES_DIR" -maxdepth 1 -mindepth 1 -type d -name 'backup-*' -printf '  %f\n' 2>/dev/null | sort || true
    fi
}

rollback_latest() {
    mkdir -p "$UPDATES_DIR"
    local backup=""
    backup="$(find "$UPDATES_DIR" -maxdepth 1 -mindepth 1 -type d -name 'backup-*' -printf '%T@ %p\n' 2>/dev/null | sort -nr | head -n 1 | cut -d' ' -f2-)"
    [[ -n "$backup" && -d "$backup" ]] || die "no update backup available"

    local old_version=""
    old_version="$(package_version "$backup/Cargo.toml" 2>/dev/null || true)"
    [[ -n "$old_version" ]] || die "backup is invalid: $backup"

    local stamp swap p
    stamp="$(date '+%Y%m%d-%H%M%S')"
    swap="$UPDATES_DIR/rollback-from-v$(current_version)-$stamp"
    mkdir -p "$swap"

    while IFS= read -r p; do
        validate_manifest_path "$p"
        if [[ -e "$ROOT/$p" || -L "$ROOT/$p" ]]; then
            mkdir -p "$swap/$(dirname -- "$p")"
            mv -- "$ROOT/$p" "$swap/$p"
        fi
    done < <(manifest_paths "$ROOT/.release-manifest")
    if [[ -f "$ROOT/.release-manifest" ]]; then
        mv -- "$ROOT/.release-manifest" "$swap/.release-manifest"
    fi

    while IFS= read -r p; do
        validate_manifest_path "$p"
        if [[ -e "$backup/$p" || -L "$backup/$p" ]]; then
            mkdir -p "$ROOT/$(dirname -- "$p")"
            mv -- "$backup/$p" "$ROOT/$p"
        fi
    done < <(manifest_paths "$backup/.release-manifest")
    if [[ -f "$backup/.release-manifest" ]]; then
        mv -- "$backup/.release-manifest" "$ROOT/.release-manifest"
    fi

    rmdir "$backup" 2>/dev/null || true
    printf 'rolled back to v%s\n' "$old_version"
    printf 'previous current source saved in: %s\n' "$swap"
}

MODE="update"
FORCE=0
DRY_RUN=0
ARCHIVE=""

while (($#)); do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        --status) MODE="status"; shift ;;
        --rollback) MODE="rollback"; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        --force) FORCE=1; shift ;;
        --*) die "unknown option: $1" ;;
        *)
            [[ -z "$ARCHIVE" ]] || die "only one release archive may be specified"
            ARCHIVE="$1"
            shift
            ;;
    esac
done

[[ -f "$ROOT/Cargo.toml" ]] || die "project Cargo.toml is missing"
[[ "$(package_name "$ROOT/Cargo.toml")" == "$PROJECT_NAME" ]] || die "unexpected project in $ROOT"

case "$MODE" in
    status) show_status; exit 0 ;;
    rollback) rollback_latest; exit 0 ;;
esac

command -v unzip >/dev/null 2>&1 || die "unzip is required"
command -v sort >/dev/null 2>&1 || die "sort is required"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"

if [[ -z "$ARCHIVE" ]]; then
    ARCHIVE="$(latest_archive || true)"
    [[ -n "$ARCHIVE" ]] || die "no ${ARCHIVE_PREFIX}*.zip found in $DEFAULT_DOWNLOAD_DIR"
fi
ARCHIVE="$(cd -- "$(dirname -- "$ARCHIVE")" && pwd -P)/$(basename -- "$ARCHIVE")"
[[ -f "$ARCHIVE" ]] || die "archive not found: $ARCHIVE"

mkdir -p "$UPDATES_DIR"
TMP="$(mktemp -d "$UPDATES_DIR/unpack.XXXXXX")"
cleanup() { rm -rf -- "$TMP" 2>/dev/null || true; }
trap cleanup EXIT

# Validate all entries BEFORE extraction, including symlinks and duplicate names.
command -v python3 >/dev/null 2>&1 || die "python3 is required"
python3 - "$ARCHIVE" <<'ZIPCHECK'
import sys, zipfile, stat
from pathlib import PurePosixPath
with zipfile.ZipFile(sys.argv[1]) as archive:
    names=set()
    for entry in archive.infolist():
        p=PurePosixPath(entry.filename)
        mode=entry.external_attr >> 16
        if (p.is_absolute() or '..' in p.parts or not p.parts
                or '\\' in entry.filename or entry.filename in names
                or stat.S_ISLNK(mode) or (stat.S_IFMT(mode) not in (0,stat.S_IFREG,stat.S_IFDIR))):
            raise SystemExit('unsafe/duplicate archive member: '+entry.filename)
        names.add(entry.filename)
ZIPCHECK
unzip -q -- "$ARCHIVE" -d "$TMP"
if [[ -n "$(find "$TMP" -type l -print -quit)" ]]; then
    die "release archive contains symlinks"
fi
RELEASE_ROOT="$(find_release_root "$TMP" || true)"
[[ -n "$RELEASE_ROOT" ]] || die "archive does not contain a $PROJECT_NAME Cargo project"
[[ -f "$RELEASE_ROOT/.release-manifest" ]] || die "release has no .release-manifest"
[[ -f "$RELEASE_ROOT/update.sh" ]] || die "release has no update.sh"
[[ -f "$RELEASE_ROOT/src/lib.rs" ]] || die "release source is incomplete"
[[ -f "$RELEASE_ROOT/SOURCE_SHA256SUMS.txt" ]] || die "release has no SOURCE_SHA256SUMS.txt"
if ! (cd "$RELEASE_ROOT" && sha256sum -c SOURCE_SHA256SUMS.txt >/dev/null); then
    die "release source checksum verification failed"
fi

# Reject untracked payload files even if the updater would otherwise ignore
# them. A release ZIP must be a manifest/checksum-owned source artifact, not a
# snapshot of a dirty working directory.
EXPECTED_FILES="$TMP/expected-files.txt"
ACTUAL_FILES="$TMP/actual-files.txt"
sed -e 's#^[0-9a-fA-F]\{64\}[[:space:]][[:space:]]*\./##' "$RELEASE_ROOT/SOURCE_SHA256SUMS.txt" | sort > "$EXPECTED_FILES"
printf '%s\n' 'SOURCE_SHA256SUMS.txt' >> "$EXPECTED_FILES"
sort -u -o "$EXPECTED_FILES" "$EXPECTED_FILES"
(cd "$RELEASE_ROOT" && find . -type f -print | sed 's#^\./##' | sort) > "$ACTUAL_FILES"
if ! diff -u "$EXPECTED_FILES" "$ACTUAL_FILES" >/dev/null; then
    die "release archive contains unowned or missing files"
fi

CURRENT_VERSION="$(current_version)"
NEW_VERSION="$(package_version "$RELEASE_ROOT/Cargo.toml")"
[[ -n "$NEW_VERSION" ]] || die "cannot determine release version"

if ((FORCE == 0)); then
    [[ "$NEW_VERSION" != "$CURRENT_VERSION" ]] || die "already on v$CURRENT_VERSION (use --force to reinstall)"
    if version_is_older "$NEW_VERSION" "$CURRENT_VERSION"; then
        die "refusing downgrade v$CURRENT_VERSION -> v$NEW_VERSION (use --force)"
    fi
fi

printf '%s update\n' "$PROJECT_NAME"
printf 'current : v%s\n' "$CURRENT_VERSION"
printf 'new     : v%s\n' "$NEW_VERSION"
printf 'archive : %s\n' "$ARCHIVE"
printf 'preserve: target/ .git/ books/ tablebases/ data/ logs/ unknown user files\n'
printf 'managed :\n'
while IFS= read -r p; do
    validate_manifest_path "$p"
    printf '  %s\n' "$p"
done < <(manifest_paths "$RELEASE_ROOT/.release-manifest")

if ((DRY_RUN)); then
    printf 'dry-run : OK; no files changed\n'
    exit 0
fi

while IFS= read -r p; do
    validate_manifest_path "$p"
done < <(manifest_paths "$ROOT/.release-manifest")

STAMP="$(date '+%Y%m%d-%H%M%S')"
BACKUP="$UPDATES_DIR/backup-v${CURRENT_VERSION}-$STAMP"
mkdir "$BACKUP"
COMMITTED=0
BACKUP_READY=0

restore_on_error() {
    local rc=$?
    if ((rc != 0 && COMMITTED == 0 && BACKUP_READY == 1)); then
        printf 'update failed; restoring v%s source...\n' "$CURRENT_VERSION" >&2
        local p
        while IFS= read -r p; do
            [[ -n "$p" ]] || continue
            rm -rf -- "$ROOT/$p" 2>/dev/null || true
        done < <(manifest_paths "$RELEASE_ROOT/.release-manifest")
        rm -f -- "$ROOT/.release-manifest" 2>/dev/null || true
        while IFS= read -r p; do
            [[ -n "$p" ]] || continue
            if [[ -e "$BACKUP/$p" || -L "$BACKUP/$p" ]]; then
                mkdir -p "$ROOT/$(dirname -- "$p")"
                mv -- "$BACKUP/$p" "$ROOT/$p" || true
            fi
        done < <(manifest_paths "$BACKUP/.release-manifest")
        if [[ -f "$BACKUP/.release-manifest" ]]; then
            mv -- "$BACKUP/.release-manifest" "$ROOT/.release-manifest" || true
        fi
    fi
    if ((rc != 0 && BACKUP_READY == 0)); then
        rm -rf -- "$BACKUP"
    fi
    cleanup
    exit "$rc"
}
trap restore_on_error ERR

# Back up only release-owned paths. Everything else remains untouched.
while IFS= read -r p; do
    validate_manifest_path "$p"
    if [[ -e "$ROOT/$p" || -L "$ROOT/$p" ]]; then
        mkdir -p "$BACKUP/$(dirname -- "$p")"
        cp -a -- "$ROOT/$p" "$BACKUP/$p"
    fi
done < <(manifest_paths "$ROOT/.release-manifest")
if [[ -f "$ROOT/.release-manifest" ]]; then
    cp -p -- "$ROOT/.release-manifest" "$BACKUP/.release-manifest"
else
    manifest_paths /nonexistent > "$BACKUP/.release-manifest"
fi

BACKUP_READY=1
# Only mutate installed sources after the entire backup is complete.
while IFS= read -r p; do
    rm -rf -- "$ROOT/$p"
done < <(manifest_paths "$BACKUP/.release-manifest")

# Install the new release-owned paths.
while IFS= read -r p; do
    validate_manifest_path "$p"
    [[ -e "$RELEASE_ROOT/$p" || -L "$RELEASE_ROOT/$p" ]] || die "release manifest path is missing: $p"
    mkdir -p "$ROOT/$(dirname -- "$p")"
    cp -a -- "$RELEASE_ROOT/$p" "$ROOT/$p"
done < <(manifest_paths "$RELEASE_ROOT/.release-manifest")
cp -p -- "$RELEASE_ROOT/.release-manifest" "$ROOT/.release-manifest"
chmod +x "$ROOT/update.sh"

INSTALLED_VERSION="$(current_version)"
[[ "$INSTALLED_VERSION" == "$NEW_VERSION" ]] || die "installed version check failed"

if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$ARCHIVE" > "$UPDATES_DIR/last-archive.sha256"
fi
printf '%s\n' "$ARCHIVE" > "$UPDATES_DIR/last-archive.path"
printf '%s\n' "$NEW_VERSION" > "$UPDATES_DIR/current-version"

COMMITTED=1
trap - ERR
printf 'updated : v%s -> v%s\n' "$CURRENT_VERSION" "$NEW_VERSION"
printf 'backup  : %s\n' "$BACKUP"
printf 'next    : cargo test --release\n'
