#!/bin/bash
# Create an R2 bucket and an API token that can only touch that bucket, with
# the Cloudflare CLI (`cf`, logged in), then connect Vroom to it.
#
#   scripts/setup-r2.sh [bucket] [--public] [--location weur]
#
# --public turns on the bucket's r2.dev URL: share links then open a page with
# the video and never expire. r2.dev is rate-limited; for heavy use attach a
# custom domain in the dashboard and set it in Vroom › Ajustes › Cloudflare R2.
# Without --public, links are signed and last 7 days.
set -euo pipefail

bucket="vroom"
public=false
location=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --public) public=true ;;
    --location) location="$2"; shift ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) bucket="$1" ;;
  esac
  shift
done

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
vroom="${VROOM_BIN:-}"
for c in "$vroom" /Applications/Vroom.app/Contents/Resources/vroom "$root/build/Vroom.app/Contents/Resources/vroom" \
  "$root/target/release/vroom"; do
  if [[ -n "$c" && -x "$c" ]]; then vroom="$c"; break; fi
done
[[ -x "$vroom" ]] || { echo "vroom not found: run 'make app' first" >&2; exit 1; }
command -v cf >/dev/null || { echo "cf not found: npm i -g cf && cf auth login" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq not found: brew install jq" >&2; exit 1; }

account="${CLOUDFLARE_ACCOUNT_ID:-$(cf accounts list | jq -r '.[0].id')}"
export CLOUDFLARE_ACCOUNT_ID="$account"
echo "==> Account $account, bucket $bucket"

if cf r2 buckets list --name-contains "$bucket" | jq -e --arg b "$bucket" '(.buckets // .) | any(.name == $b)' >/dev/null; then
  echo "==> Bucket exists"
else
  echo "==> Creating bucket"
  args=(--name "$bucket")
  [[ -n "$location" ]] && args+=(--location-hint "$location")
  cf r2 buckets create "${args[@]}" >/dev/null
fi

# S3 credentials for R2 are an API token: the token id is the Access Key ID and
# the SHA-256 of its value is the Secret. The response holds the value, so it
# goes to a private temp file and never to the terminal.
group="$(cf accounts tokens permission-groups list | jq -r '.[] | select(.name == "Workers R2 Storage Bucket Item Write") | .id')"
[[ -n "$group" ]] || { echo "R2 write permission group not found" >&2; exit 1; }
policies="$(jq -cn --arg g "$group" --arg r "com.cloudflare.edge.r2.bucket.${account}_default_${bucket}" \
  '[{effect: "allow", permission_groups: [{id: $g}], resources: {($r): "*"}}]')"
tmp="$(mktemp)"
chmod 600 "$tmp"
trap 'rm -f "$tmp"' EXIT
echo "==> Creating an API token limited to $bucket"
cf accounts tokens create --name "vroom-$bucket-$(hostname -s)" --policies "$policies" > "$tmp"
key_id="$(jq -r '.id // .result.id' "$tmp")"
secret="$(jq -r '.value // .result.value' "$tmp" | tr -d '\n' | shasum -a 256 | cut -d' ' -f1)"
rm -f "$tmp"

public_args=()
if $public; then
  echo "==> Turning on the r2.dev public URL"
  domain="$(cf r2 buckets domains managed update "$bucket" --enabled true --force | jq -r '.domain // .result.domain')"
  public_args=(--public-url "https://$domain")
fi

echo "==> Connecting Vroom (new tokens can take a few seconds to work)"
for attempt in 1 2 3 4 5 6; do
  if printf '%s\n' "$secret" | "$vroom" config r2 --account-id "$account" --bucket "$bucket" \
    --access-key-id "$key_id" ${public_args[@]+"${public_args[@]}"}; then
    exit 0
  fi
  sleep $((attempt * 3))
done
exit 1
