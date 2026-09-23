#!/usr/bin/env bash

# Notarize a standalone binary with xcrun notarytool using an App Store Connect
# API key and retain its diagnostic report.

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: notarize_macos_binary.sh --binary PATH [--report-dir PATH] [--max-wait-seconds SECONDS]

Options:
  --binary PATH                 Signed standalone macOS binary to notarize.
  --report-dir PATH             Directory for notarization logs.
  --max-wait-seconds SECONDS    Maximum Apple notarization wait time.
EOF
}

binary_path=""
report_dir="${RUNNER_TEMP:-/tmp}/macos-binary-notarization-verification"
max_wait_seconds="600"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary)
      binary_path="${2:-}"
      shift 2
      ;;
    --report-dir)
      report_dir="${2:-}"
      shift 2
      ;;
    --max-wait-seconds)
      max_wait_seconds="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown notarization argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$binary_path" ]]; then
  echo "--binary is required." >&2
  usage
  exit 2
fi

if [[ ! -f "$binary_path" ]]; then
  echo "Binary does not exist: $binary_path" >&2
  exit 1
fi

if [[ ! "$max_wait_seconds" =~ ^[0-9]+$ ]]; then
  echo "--max-wait-seconds must be a non-negative integer." >&2
  exit 2
fi

for command_name in xcrun zip; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "$command_name was not found on PATH." >&2
    exit 1
  fi
done

missing_environment=0
for variable_name in AC_API_KEY AC_API_KEY_ID AC_API_ISSUER_ID; do
  if [[ -z "${!variable_name:-}" ]]; then
    echo "$variable_name must be configured before notarizing a binary." >&2
    missing_environment=1
  fi
done

if [[ "$missing_environment" -ne 0 ]]; then
  exit 2
fi

mkdir -p "$report_dir"

notarization_temp_dir="$(mktemp -d)"
api_key_file="$(mktemp /tmp/notarization-api-key.XXXXXXXX.p8)"
trap 'rm -rf "$notarization_temp_dir"; rm -f "$api_key_file"' EXIT

binary_name="$(basename "$binary_path")"
archive_path="$notarization_temp_dir/${binary_name}.zip"
(
  cd "$(dirname "$binary_path")"
  zip -q "$archive_path" "$binary_name"
)

printf '%s' "$AC_API_KEY" | base64 --decode >"$api_key_file"
chmod 0600 "$api_key_file"

notarization_log="$report_dir/${binary_name}-notarization.log"
xcrun notarytool submit "$archive_path" \
  --key "$api_key_file" \
  --key-id "$AC_API_KEY_ID" \
  --issuer "$AC_API_ISSUER_ID" \
  --wait \
  --output-format plist \
  2>&1 | tee "$notarization_log"

{
  echo "binary_name=$binary_name"
  echo "max_wait_seconds=$max_wait_seconds"
  echo "binary_sha256=$(shasum -a 256 "$binary_path" | awk '{ print $1 }')"
  echo "notarization=completed"
} >"$report_dir/${binary_name}-notarization-summary.txt"