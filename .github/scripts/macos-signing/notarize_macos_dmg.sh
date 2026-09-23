#!/usr/bin/env bash

# Notarize a signed disk image and staple its ticket.

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: notarize_macos_dmg.sh --dmg PATH [--report-dir PATH] [--max-wait-seconds SECONDS]

Options:
  --dmg PATH                    Signed DMG to submit to Apple notarization.
  --report-dir PATH             Directory for notarization logs.
  --max-wait-seconds SECONDS    Maximum Apple notarization wait time.
EOF
}

dmg_path=""
report_dir="${RUNNER_TEMP:-/tmp}/macos-notarization-verification"
max_wait_seconds="600"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dmg)
      dmg_path="${2:-}"
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

if [[ -z "$dmg_path" ]]; then
  echo "--dmg is required." >&2
  usage
  exit 2
fi

if [[ ! -f "$dmg_path" ]]; then
  echo "DMG does not exist: $dmg_path" >&2
  exit 1
fi

if [[ ! "$max_wait_seconds" =~ ^[0-9]+$ ]]; then
  echo "--max-wait-seconds must be a non-negative integer." >&2
  exit 2
fi

if ! command -v xcrun >/dev/null 2>&1; then
  echo "xcrun was not found on PATH." >&2
  exit 1
fi

missing_environment=0
for variable_name in AC_API_KEY AC_API_KEY_ID AC_API_ISSUER_ID; do
  if [[ -z "${!variable_name:-}" ]]; then
    echo "$variable_name must be configured before notarizing a DMG." >&2
    missing_environment=1
  fi
done

if [[ "$missing_environment" -ne 0 ]]; then
  exit 2
fi

mkdir -p "$report_dir"

api_key_file="$(mktemp /tmp/notarization-api-key.XXXXXXXX.p8)"
trap 'rm -f "$api_key_file" >/dev/null' EXIT
printf '%s' "$AC_API_KEY" | base64 --decode >"$api_key_file"
chmod 0600 "$api_key_file"

notarization_log="$report_dir/dmg-notarization.log"
xcrun notarytool submit "$dmg_path" \
  --key "$api_key_file" \
  --key-id "$AC_API_KEY_ID" \
  --issuer "$AC_API_ISSUER_ID" \
  --wait \
  --output-format plist \
  2>&1 | tee "$notarization_log"

xcrun stapler staple "$dmg_path" 2>&1 | tee -a "$notarization_log"

{
  echo "dmg_path=$dmg_path"
  echo "max_wait_seconds=$max_wait_seconds"
  echo "dmg_sha256=$(shasum -a 256 "$dmg_path" | awk '{ print $1 }')"
  echo "notarization_staple=completed"
} >"$report_dir/dmg-notarization-summary.txt"