#!/usr/bin/env bash
# Smoke test against the Compose stack: get Keycloak tokens, use both APIs through HAProxy.
set -euo pipefail
# Pick up port overrides from deploy/.env, same as docker compose does.
ENV_FILE="$(dirname "$0")/../deploy/.env"
[ -f "$ENV_FILE" ] && set -a && . "$ENV_FILE" && set +a
KC=${KC:-http://localhost:${CC_KEYCLOAK_PORT:-18081}/realms/kv/protocol/openid-connect/token}
LB_HTTP=${LB_HTTP:-http://localhost:${CC_HTTP_PORT:-18080}}
LB_TCP_HOST=${LB_TCP_HOST:-localhost}
LB_TCP_PORT=${LB_TCP_PORT:-${CC_TCP_PORT:-17379}}

token() { # user password [scope]
  curl -fsS "$KC" -d grant_type=password -d client_id=kv-cli -d username="$1" -d password="$2" \
    ${3:+-d "scope=$3"} | sed -E 's/.*"access_token":"([^"]+)".*/\1/'
}

ALICE=$(token alice alice)   # acme, kv-write
BOB=$(token bob bob)         # acme, kv-read
CAROL=$(token carol carol)   # globex, kv-write
DAVE=$(token dave dave "openid organization:*")   # acme + globex: must pick per request
WORKER=$(curl -fsS "$KC" -d grant_type=client_credentials -d client_id=kv-worker \
  -d client_secret=change-me-kv-worker | sed -E 's/.*"access_token":"([^"]+)".*/\1/')

echo "== HTTP"
curl -fsS -X PUT "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $ALICE" --data 'hello from alice' -o /dev/null -w 'alice PUT  -> %{http_code}\n'
curl -sS  -X PUT "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $BOB"   --data 'nope'            -o /dev/null -w 'bob PUT    -> %{http_code} (expect 403)\n'
echo -n "bob GET    -> "; curl -fsS "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $BOB"; echo
curl -fsS -X PUT "$LB_HTTP/v1/kv/job:1?ttl=30" -H "Authorization: Bearer $WORKER" --data 'queued' -o /dev/null -w 'worker PUT -> %{http_code} (client credentials, 30s TTL)\n'

echo "== Organizations"
curl -fsS -X PUT "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $CAROL" --data 'hello from globex' -o /dev/null -w 'carol PUT  -> %{http_code} (same key, other org)\n'
echo -n "bob GET    -> "; curl -fsS "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $BOB"; echo "   (still acme's value)"
curl -sS "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $DAVE" -o /dev/null -w 'dave GET   -> %{http_code} (in two orgs, none chosen)\n'
echo -n "dave GET   -> "; curl -fsS "$LB_HTTP/v1/kv/greeting" -H "Authorization: Bearer $DAVE" -H 'X-Org-Id: globex'; echo "   (X-Org-Id: globex)"

echo "== TCP"
printf 'PING\nAUTH %s\nSET session:42 logged in\nGET session:42\nQUIT\n' "$ALICE" \
  | nc -q 2 "$LB_TCP_HOST" "$LB_TCP_PORT" 2>/dev/null || \
printf 'PING\nAUTH %s\nSET session:42 logged in\nGET session:42\nQUIT\n' "$ALICE" \
  | nc "$LB_TCP_HOST" "$LB_TCP_PORT"

echo "== Usage (role kv-usage, held by the dashboard's service account)"
USAGE=$(curl -fsS "$KC" -d grant_type=client_credentials -d client_id=kv-dashboard \
  -d client_secret=change-me-kv-dashboard | sed -E 's/.*"access_token":"([^"]+)".*/\1/')
# Through the load balancer this shows one node's counters; the dashboard asks each node and sums.
curl -fsS "$LB_HTTP/v1/admin/usage" -H "Authorization: Bearer $USAGE"; echo
