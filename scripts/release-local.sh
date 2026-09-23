#!/usr/bin/env bash
#
# Local macOS release driver for Sofia.
#
# Bumps the version, builds, signs, notarizes, packages, publishes a GitHub
# Release, and mirrors the release to an S3-compatible bucket (Contabo, R2, ...)
# using the same scripts the CI release workflow uses. Intended for macOS hosts
# when GitHub-hosted runners are unavailable.
#
# Usage:
#   scripts/release-local.sh --bump patch [options]
#   scripts/release-local.sh --version 0.1.1 [options]
#
# Options:
#   --version X.Y.Z        Explicit version to release (tag rust-vX.Y.Z).
#   --bump patch|minor|major
#                          Derive the next version from the current one.
#   --target TARGET        Rust target (default: native macOS target).
#                          Repeatable. Only *-apple-darwin targets are supported.
#   --env-file PATH        File to source before running (default:
#                          $HOME/.sofia/release.env if it exists).
#   --work-dir PATH        Scratch dir (default: .release-work).
#   --dist-dir PATH        Staging dir for release assets (default: dist-local).
#   --skip-bump            Do not change the version.
#   --skip-commit          Do not commit or tag the version bump.
#   --skip-push            Do not push the commit/tag.
#   --skip-build           Reuse previously built binaries.
#   --skip-sign            Reuse previously signed binaries.
#   --skip-notarize        Do not submit to Apple notarization.
#   --skip-package         Do not rebuild package archives.
#   --skip-dmg             Do not build the DMG.
#   --skip-github-release  Do not create/update the GitHub Release.
#   --skip-s3              Do not publish to the S3 mirror.
#   --dry-run              Print commands without executing them.
#   -h, --help             Show this help.
#
# Required environment (via --env-file or the ambient environment):
#   Signing:  MACOS_CERTIFICATE (base64 .p12) + MACOS_CERTIFICATE_PWD,
#             or MACOS_P12_PATH + MACOS_CERTIFICATE_PWD,
#             or an existing "Developer ID Application" identity in a keychain.
#   Notary:   AC_API_KEY (base64 .p8), AC_API_KEY_ID, AC_API_ISSUER_ID.
#   S3:       AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_ENDPOINT_URL,
#             AWS_REGION, SOFIA_R2_BUCKET, SOFIA_R2_PUBLIC_BASE_URL.
#   GitHub:   GH_TOKEN or an authenticated `gh` (used for the release + mirror).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION=""
BUMP=""
TARGETS=()
ENV_FILE="${HOME}/.sofia/release.env"
WORK_DIR="${REPO_ROOT}/.release-work"
DIST_DIR="${REPO_ROOT}/dist-local"
SKIP_BUMP=0
SKIP_COMMIT=0
SKIP_PUSH=0
SKIP_BUILD=0
SKIP_SIGN=0
SKIP_NOTARIZE=0
SKIP_PACKAGE=0
SKIP_DMG=0
SKIP_GITHUB=0
SKIP_S3=0
DRY_RUN=0

CREATED_KEYCHAIN=""
DIST_DIR_ABS=""
REPOSITORY=""

log() { printf '\n\033[1m==> %s\033[0m\n' "$*" >&2; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
run() {
  if [[ "$DRY_RUN" == "1" ]]; then
    printf '+'; printf ' %q' "$@"; printf '\n'
  else
    "$@"
  fi
}

usage() { sed -n '2,60p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) VERSION="${2:?--version requires a value}"; shift 2 ;;
    --bump) BUMP="${2:?--bump requires a value}"; shift 2 ;;
    --target) TARGETS+=("${2:?--target requires a value}"); shift 2 ;;
    --env-file) ENV_FILE="${2:?--env-file requires a value}"; shift 2 ;;
    --work-dir) WORK_DIR="${2:?--work-dir requires a value}"; shift 2 ;;
    --dist-dir) DIST_DIR="${2:?--dist-dir requires a value}"; shift 2 ;;
    --skip-bump) SKIP_BUMP=1; shift ;;
    --skip-commit) SKIP_COMMIT=1; shift ;;
    --skip-push) SKIP_PUSH=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-sign) SKIP_SIGN=1; shift ;;
    --skip-notarize) SKIP_NOTARIZE=1; shift ;;
    --skip-package) SKIP_PACKAGE=1; shift ;;
    --skip-dmg) SKIP_DMG=1; shift ;;
    --skip-github-release) SKIP_GITHUB=1; shift ;;
    --skip-s3) SKIP_S3=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
done

cleanup() {
  if [[ -n "$CREATED_KEYCHAIN" ]]; then
    security delete-keychain "$CREATED_KEYCHAIN" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Environment + prerequisites
# ---------------------------------------------------------------------------

if [[ -f "$ENV_FILE" ]]; then
  log "Loading environment from $ENV_FILE"
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

[[ "$(uname -s)" == "Darwin" ]] || die "local macOS releases must run on macOS"

if ((${#TARGETS[@]} == 0)); then
  case "$(uname -m)" in
    arm64) TARGETS=("aarch64-apple-darwin") ;;
    x86_64) TARGETS=("x86_64-apple-darwin") ;;
    *) die "unsupported host arch: $(uname -m)" ;;
  esac
fi
for target in "${TARGETS[@]}"; do
  [[ "$target" == *-apple-darwin ]] || die "only *-apple-darwin targets are supported, got: $target"
done

for tool in cargo rustup git gh codesign xcrun hdiutil zstd security; do
  command -v "$tool" >/dev/null 2>&1 || die "missing required tool: $tool"
done
command -v aws >/dev/null 2>&1 || warn "'aws' CLI not found; S3 publishing will fail until installed (brew install awscli)"

resolve_python() {
  local candidates=()
  [[ -n "${SOFIA_PYTHON:-}" ]] && candidates+=("$SOFIA_PYTHON")
  candidates+=(python3.13 python3.12 python3.11 python3.10 python3)
  [[ -x "${REPO_ROOT}/scripts/.venv/bin/python" ]] && candidates+=("${REPO_ROOT}/scripts/.venv/bin/python")
  local candidate
  for candidate in "${candidates[@]}"; do
    if command -v "$candidate" >/dev/null 2>&1 \
      && "$candidate" -c 'import sys; sys.exit(0 if sys.version_info[:2] >= (3, 10) else 1)' 2>/dev/null; then
      command -v "$candidate"
      return 0
    fi
  done
  return 1
}
PYTHON="$(resolve_python)" || die "python >= 3.10 not found; set SOFIA_PYTHON"

mkdir -p "$WORK_DIR"
DIST_DIR_ABS="$(cd "$(dirname "$DIST_DIR")" 2>/dev/null && pwd)/$(basename "$DIST_DIR")" 2>/dev/null || DIST_DIR_ABS="$DIST_DIR"

# Some helpers hardcode `python3`; shim it to the resolved interpreter so they
# do not pick up an older system Python.
mkdir -p "${WORK_DIR}/bin"
ln -sf "$PYTHON" "${WORK_DIR}/bin/python3"
ln -sf "$PYTHON" "${WORK_DIR}/bin/python"
export PATH="${WORK_DIR}/bin:${PATH}"

# ---------------------------------------------------------------------------
# Version bump
# ---------------------------------------------------------------------------

read_workspace_version() {
  grep -m1 '^version' "${REPO_ROOT}/sofia-rs/Cargo.toml" \
    | sed -E 's/version *= *"([^"]+)".*/\1/'
}

bump_version() {
  local current="$1" part="$2" major minor patch
  IFS=. read -r major minor patch <<<"$current"
  case "$part" in
    major) major=$((major + 1)); minor=0; patch=0 ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    patch) patch=$((patch + 1)) ;;
    *) die "--bump must be patch, minor, or major (got '$part')" ;;
  esac
  printf '%s.%s.%s\n' "$major" "$minor" "$patch"
}

if [[ "$SKIP_BUMP" == "0" ]]; then
  current_version="$(read_workspace_version)"
  if [[ -n "$BUMP" ]]; then
    VERSION="$(bump_version "$current_version" "$BUMP")"
  fi
  [[ -n "$VERSION" ]] || die "provide --version X.Y.Z or --bump patch|minor|major"
  [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be stable X.Y.Z (got '$VERSION')"
  if [[ "$VERSION" != "$current_version" ]]; then
    log "Bumping version $current_version -> $VERSION"
    run "$PYTHON" - "$VERSION" "$current_version" <<'PY'
import pathlib
import sys

new, old = sys.argv[1], sys.argv[2]
root = pathlib.Path.cwd()
cargo_toml = root / "sofia-rs" / "Cargo.toml"
lock = root / "sofia-rs" / "Cargo.lock"
text = cargo_toml.read_text()
marker = f'version = "{old}"'
count = text.count("\n" + marker)
if count != 1:
    raise SystemExit(f"expected exactly one workspace version line, found {count}")
cargo_toml.write_text(text.replace("\n" + marker, f'\nversion = "{new}"', 1))
lock_text = lock.read_text()
lock.write_text(lock_text.replace(f'version = "{old}"', f'version = "{new}"'))
print(f"updated Cargo.toml and Cargo.lock to {new}")
PY
  else
    log "Version already $VERSION; no bump needed"
  fi
else
  VERSION="$(read_workspace_version)"
  log "Using existing workspace version $VERSION"
fi
TAG="rust-v${VERSION}"

if [[ "$SKIP_COMMIT" == "0" && "$DRY_RUN" == "0" ]]; then
  if ! git diff --quiet -- sofia-rs/Cargo.toml sofia-rs/Cargo.lock; then
    run git add sofia-rs/Cargo.toml sofia-rs/Cargo.lock
    run git commit -m "chore(release): bump version to ${VERSION}"
  fi
  if ! git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
    run git tag -a "$TAG" -m "Release ${VERSION}"
  fi
  if [[ "$SKIP_PUSH" == "0" ]]; then
    run git push origin HEAD
    run git push origin "$TAG"
  fi
fi

# ---------------------------------------------------------------------------
# Signing identity / notarization credentials
# ---------------------------------------------------------------------------

SIGN="${REPO_ROOT}/.github/scripts/macos-signing"
if [[ "$SKIP_SIGN" == "0" || "$SKIP_DMG" == "0" ]]; then
  IDENTITY="${SOFIA_SIGN_IDENTITY:-}"
  if [[ -z "$IDENTITY" ]]; then
    p12_path=""
    if [[ -n "${MACOS_CERTIFICATE:-}" ]]; then
      p12_path="${WORK_DIR}/developer-id.p12"
      log "Importing MACOS_CERTIFICATE into a temporary keychain"
      run bash -c "printf '%s' \"\$MACOS_CERTIFICATE\" | base64 --decode > '$p12_path'"
    elif [[ -n "${MACOS_P12_PATH:-}" ]]; then
      p12_path="$MACOS_P12_PATH"
    fi

    if [[ -n "$p12_path" ]]; then
      [[ -n "${MACOS_CERTIFICATE_PWD:-}" ]] || die "MACOS_CERTIFICATE_PWD is required to import the p12"
      CREATED_KEYCHAIN="${WORK_DIR}/sofia-release.keychain-db"
      run security delete-keychain "$CREATED_KEYCHAIN" || true
      run security create-keychain -p sofia-release "$CREATED_KEYCHAIN"
      run security set-keychain-settings -lut 21600 "$CREATED_KEYCHAIN"
      run security unlock-keychain -p sofia-release "$CREATED_KEYCHAIN"
      run security import "$p12_path" -P "$MACOS_CERTIFICATE_PWD" -A -t cert -f pkcs12 -k "$CREATED_KEYCHAIN"
      run security import "$p12_path" -P "$MACOS_CERTIFICATE_PWD" -A -t agg -f pkcs12 -k "$CREATED_KEYCHAIN"
      run security list-keychain -d user -s "$CREATED_KEYCHAIN"
    fi

    IDENTITY="$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/ { print $2; exit }')"
  fi
  [[ -n "$IDENTITY" ]] || die "no 'Developer ID Application' identity found"
  log "Signing identity: $IDENTITY"

  if [[ "$SKIP_NOTARIZE" == "0" ]]; then
    for var in AC_API_KEY AC_API_KEY_ID AC_API_ISSUER_ID; do
      [[ -n "${!var:-}" ]] || die "$var is required for notarization"
    done
  fi
fi

sign_binary() { # path, identifier, [entitlements]
  local path="$1" identifier="$2" ent="${3:-}"
  local args=(--target "$path" --identity "$IDENTITY" --deep false
    --identifier "$identifier" --options runtime --timestamp true)
  [[ -n "$ent" ]] && args+=(--entitlements "$ent")
  run "$SIGN/sign_macos_code.sh" "${args[@]}"
}

notarize_binary() { # path, report_subdir
  [[ "$SKIP_NOTARIZE" == "1" ]] && return 0
  run "$SIGN/notarize_macos_binary.sh" --binary "$1" --report-dir "$WORK_DIR/reports/$2"
}

# ---------------------------------------------------------------------------
# Helpers (rg + zsh) fetched from their DotSlash manifests
# ---------------------------------------------------------------------------

fetch_helpers() { # target, out_dir
  local target="$1" out="$2"
  mkdir -p "$out"
  run env PYTHONPATH="${REPO_ROOT}/scripts" "$PYTHON" - "$target" "$out" "$REPO_ROOT/scripts/sofia_package/sofia-zsh" <<'PY'
import shutil
import sys
from pathlib import Path

from sofia_package.ripgrep import fetch_rg
from sofia_package.targets import TARGET_SPECS
from sofia_package.zsh import resolve_zsh_bin

spec = TARGET_SPECS[sys.argv[1]]
out = Path(sys.argv[2])
zsh_bin = resolve_zsh_bin(spec, Path(sys.argv[3]))
if zsh_bin is None:
    raise SystemExit(f"zsh manifest has no binary for {spec.target}")
shutil.copy2(fetch_rg(spec), out / "rg")
shutil.copy2(zsh_bin, out / "zsh")
for name in ("rg", "zsh"):
    (out / name).chmod(0o755)
PY
}

# ---------------------------------------------------------------------------
# Build + sign + notarize per target
# ---------------------------------------------------------------------------

build_binaries() { # target
  local target="$1"
  local version archive binding base_url trusted
  log "Configuring rusty_v8 artifacts for $target"
  version="$("$PYTHON" "${REPO_ROOT}/.github/scripts/rusty_v8_bazel.py" resolved-v8-crate-version)"
  base_url="https://github.com/openai/codex/releases/download/rusty-v8-v${version}"
  local binding_dir="${WORK_DIR}/rusty_v8"
  local profile="ptrcomp_sandbox_release"
  archive="${binding_dir}/librusty_v8_${profile}_${target}.a.gz"
  binding="${binding_dir}/src_binding_${profile}_${target}.rs"
  local checksums="${binding_dir}/rusty_v8_${profile}_${target}.sha256"
  trusted="${REPO_ROOT}/third_party/v8/rusty_v8_${version//./_}_release_manifests.sha256"
  run mkdir -p "$binding_dir"
  run curl -fsSL "${base_url}/$(basename "$checksums")" -o "$checksums"
  run curl -fsSL "${base_url}/$(basename "$archive")" -o "$archive"
  run curl -fsSL "${base_url}/$(basename "$binding")" -o "$binding"

  local expected_manifest_checksum actual_manifest_checksum
  expected_manifest_checksum="$(grep -F "  $(basename "$checksums")" "$trusted" | cut -d ' ' -f 1)"
  actual_manifest_checksum="$("$PYTHON" -c 'import hashlib,pathlib,sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())' "$checksums")"
  [[ -n "$expected_manifest_checksum" && "$actual_manifest_checksum" == "$expected_manifest_checksum" ]] \
    || die "rusty_v8 checksum manifest mismatch for $target"
  run bash -c "cd '$binding_dir' && tr -d '\r' < '$checksums' | shasum -a 256 --check - >/dev/null"
  export RUSTY_V8_ARCHIVE="$archive"
  export RUSTY_V8_SRC_BINDING_PATH="$binding"

  log "Building release binaries for $target"
  export CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=packed
  STABLE_GIT_COMMIT="$(git rev-parse HEAD)"
  export STABLE_GIT_COMMIT
  run cargo build --manifest-path "${REPO_ROOT}/sofia-rs/Cargo.toml" \
    --target "$target" --release --timings \
    --bin sofia --bin sofia-code-mode-host --bin sofia-responses-api-proxy \
    --bin sofia-app-server
}

release_dir() { printf '%s/sofia-rs/target/%s/release' "$REPO_ROOT" "$1"; }
signed_dir() { printf '%s/%s' "$WORK_DIR" "$1"; }

sign_target() { # target
  local target="$1"
  local rd out
  rd="$(release_dir "$target")"
  out="$(signed_dir "$target")"
  run mkdir -p "$out" "$out/resources"

  log "Signing binaries for $target"
  local -a entries=(
    "sofia:sofia.entitlements.plist"
    "sofia-code-mode-host:sofia-code-mode-host.entitlements.plist"
    "sofia-responses-api-proxy:sofia-responses-api-proxy.entitlements.plist"
    "sofia-app-server:sofia-app-server.entitlements.plist"
  )
  for entry in "${entries[@]}"; do
    local bin="${entry%%:*}" ent="${entry##*:}"
    [[ -f "$rd/$bin" ]] || die "missing built binary: $rd/$bin"
    run cp -f "$rd/$bin" "$out/$bin"
    sign_binary "$out/$bin" "$bin" "$SIGN/$ent"
    notarize_binary "$out/$bin" "${target}/${bin}"
  done

  log "Fetching and signing helpers for $target"
  fetch_helpers "$target" "$out/resources"
  for resource in rg zsh; do
    sign_binary "$out/resources/$resource" "com.openai.sofia.$resource"
    notarize_binary "$out/resources/$resource" "${target}/${resource}"
  done
}

# ---------------------------------------------------------------------------
# Package archives + DMG
# ---------------------------------------------------------------------------

package_target() { # target
  local target="$1" out
  out="$(signed_dir "$target")"
  local archive_dir="${DIST_DIR_ABS}/${target}"
  run mkdir -p "$archive_dir"

  for bundle in primary app-server; do
    log "Assembling $bundle package archive for $target"
    run bash "${REPO_ROOT}/.github/scripts/build-sofia-package-archive.sh" \
      --target "$target" \
      --bundle "$bundle" \
      --entrypoint-dir "$out" \
      --archive-dir "$archive_dir" \
      --rg-bin "$out/resources/rg" \
      --zsh-bin "$out/resources/zsh"
  done

  for asset in sofia sofia-code-mode-host sofia-responses-api-proxy sofia-app-server; do
    cp -f "$out/$asset" "$archive_dir/${asset}-${target}"
  done
}

finalize_assets() {
  [[ -d "$DIST_DIR_ABS" ]] || return 0
  log "Staging installers and the package checksum manifest"
  run cp -f "${REPO_ROOT}/scripts/install/install.sh" "$DIST_DIR_ABS/install.sh"
  run cp -f "${REPO_ROOT}/scripts/install/install.ps1" "$DIST_DIR_ABS/install.ps1"
  run "$PYTHON" - "$DIST_DIR_ABS" <<'PY'
import hashlib
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
lines = []
for path in sorted(root.rglob("*.tar.gz")):
    if path.name.startswith(("sofia-package-", "sofia-app-server-package-")):
        lines.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")
(root / "sofia-package_SHA256SUMS").write_text("\n".join(lines) + ("\n" if lines else ""))
print(f"wrote {len(lines)} package checksums")
PY
}

build_dmg() { # target
  local target="$1" out
  out="$(signed_dir "$target")"
  local archive_dir="${DIST_DIR_ABS}/${target}"
  local dmg_root="${WORK_DIR}/dmg-root-${target}"
  local dmg_path="$archive_dir/sofia-${target}.dmg"

  log "Building DMG for $target"
  run rm -rf "$dmg_root"
  run mkdir -p "$dmg_root"
  for bin in sofia sofia-code-mode-host sofia-responses-api-proxy; do
    run ditto "$out/$bin" "$dmg_root/$bin"
  done
  run rm -f "$dmg_path"
  run hdiutil create -volname "Sofia (${target})" -srcfolder "$dmg_root" -format UDZO -ov "$dmg_path"

  sign_binary "$dmg_path" "sofia-dmg-${target}"
  if [[ "$SKIP_NOTARIZE" == "0" ]]; then
    run "$SIGN/notarize_macos_dmg.sh" --dmg "$dmg_path" --report-dir "$WORK_DIR/reports/${target}/dmg"
  fi
}

# ---------------------------------------------------------------------------
# Publish
# ---------------------------------------------------------------------------

resolve_repo() {
  if [[ -z "$REPOSITORY" ]]; then
    REPOSITORY="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null || true)"
  fi
  [[ -n "$REPOSITORY" ]] || die "could not determine the GitHub repository (set SOFIA_RELEASE_REPOSITORY)"
}

github_release() {
  resolve_repo
  local assets=()
  while IFS= read -r asset; do
    assets+=("$asset")
  done < <(find "$DIST_DIR_ABS" -type f)
  ((${#assets[@]})) || die "no release assets found under $DIST_DIR_ABS"
  log "Creating/updating GitHub Release $TAG ($REPOSITORY)"
  if gh release view "$TAG" --repo "$REPOSITORY" >/dev/null 2>&1; then
    run gh release upload "$TAG" --clobber --repo "$REPOSITORY" "${assets[@]}"
  else
    run gh release create "$TAG" --repo "$REPOSITORY" \
      --title "$VERSION" --notes "Sofia $VERSION (local macOS release)" \
      "${assets[@]}"
  fi
}

publish_s3() {
  resolve_repo
  [[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] \
    || die "AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are required for S3 publishing"
  log "Mirroring $TAG to s3://${SOFIA_R2_BUCKET:-<unset>}"
  export GH_TOKEN="${GH_TOKEN:-$(gh auth token)}"
  export SOFIA_RELEASE_REPOSITORY="$REPOSITORY"
  run "$PYTHON" "${REPO_ROOT}/.github/scripts/publish_r2_release.py" \
    --tag "$TAG" --make-latest true --prerelease false --stage assets
  run "$PYTHON" "${REPO_ROOT}/.github/scripts/publish_r2_release.py" \
    --tag "$TAG" --make-latest true --prerelease false --stage finalize
}

# ---------------------------------------------------------------------------

log "Sofia local release: version=$VERSION tag=$TAG targets=${TARGETS[*]}"

for target in "${TARGETS[@]}"; do
  rustup target list --installed | grep -qx "$target" || run rustup target add "$target"
  [[ "$SKIP_BUILD" == "1" ]] || build_binaries "$target"
  [[ "$SKIP_SIGN" == "1" ]] || sign_target "$target"
  [[ "$SKIP_PACKAGE" == "1" ]] || package_target "$target"
  [[ "$SKIP_DMG" == "1" ]] || build_dmg "$target"
done

finalize_assets

[[ "$SKIP_GITHUB" == "1" ]] || github_release
[[ "$SKIP_S3" == "1" ]] || publish_s3

log "Done. Version $VERSION (tag $TAG)."
