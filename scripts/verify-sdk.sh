#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$(pwd)"
binary="${1:-"$repo/target/debug/confuse"}"
work="$(mktemp -d)"
pid=""
cleanup() {
  if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
fixture="${CONFUSE_SDK_FIXTURE:-"$work/clients"}"
if [[ -z "${CONFUSE_SDK_FIXTURE:-}" ]]; then
  npm install --prefix "$fixture" --ignore-scripts --no-audit --no-fund @langfuse/client@6.1.1 @langfuse/otel@6.1.1 @langfuse/tracing@6.1.1 @opentelemetry/api@1.9.0 @opentelemetry/sdk-trace-node
  python3 -m venv "$fixture/python"
  "$fixture/python/bin/python" -m pip -q install 'langfuse==5.2.0'
fi
cp "$repo/scripts/sdk-js.mjs" "$fixture/verify-confuse.mjs"
key="$("$binary" key "$work/state")"
CONFUSE_PORT=0 "$binary" serve "$work/state" > "$work/url" &
pid=$!
for _ in {1..100}; do
  [[ -s "$work/url" ]] && break
  kill -0 "$pid"
  sleep .1
done
LANGFUSE_BASE_URL="$(head -n 1 "$work/url")"
[[ "$LANGFUSE_BASE_URL" == http://127.0.0.1:* ]]
LANGFUSE_PUBLIC_KEY="$(jq -r .publicKey <<<"$key")"
LANGFUSE_SECRET_KEY="$(jq -r .secretKey <<<"$key")"
export LANGFUSE_BASE_URL LANGFUSE_PUBLIC_KEY LANGFUSE_SECRET_KEY
"$fixture/python/bin/python" "$repo/scripts/sdk-python.py"
node "$fixture/verify-confuse.mjs"
