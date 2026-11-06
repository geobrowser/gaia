#!/bin/sh
# Runs geodb-serve for the geodb StatefulSet.
#
# serve just runs: geo-db founds Spaces online over the wire (FoundSpace), so nothing here restarts it.
# What this script adds around serve:
#   - the binding file, derived from the environment on every start (a rotated key id takes effect then);
#   - serve's two public ids, published to ConfigMap geodb-space-map as `node-id` (which TCP clients pin
#     in their binding) and `sealer-id` (which the feeder needs to derive a Space's id); also written to
#     $DATA/ctl for a local run;
#   - $DATA/ctl/serving once serve has printed them, which is the pod's readiness signal;
#   - a clean stop: SIGTERM is forwarded as SIGINT so serve drains and writes its final checkpoint.
set -u

DATA="${DATA_DIR:-/data}"
RUN="$DATA/run"
CTL="$DATA/ctl"
STORE="$DATA/store"
mkdir -p "$RUN" "$CTL" "$STORE"
cd "$RUN" || exit 1

: "${GEODB_WRITER_KEYID:?GEODB_WRITER_KEYID is required}"
: "${GEODB_READER_KEYID:?GEODB_READER_KEYID is required}"
: "${GEODB_STORE_SECRET_FILE:?GEODB_STORE_SECRET_FILE is required}"
CONFIG="${GEODB_CONFIG:-/etc/geodb/serve.cfg}"
PROFILE="${GEODB_PROFILE:-medium}"
PORT="${GEODB_PORT:-7447}"
GRANT_TTL="${GEODB_GRANT_TTL_S:-300}"
CONFIGMAP="${GEODB_CONFIGMAP:-geodb-space-map}"
SA=/var/run/secrets/kubernetes.io/serviceaccount

# The writer may publish (found) any Space; the reader reads and nothing else. The optional cursor key
# (hermes's meta-cursor mirror, Secret geodb-cursor-key) may write derived collection 10 and nothing
# else: no read, no graph write, no publish. Needs geodb-serve at geo-db d903c8e017 or later (`derived=`).
cat > "$RUN/binding.txt" <<BINDING
version = 1
listen = tcp:0.0.0.0:$PORT
principal = $GEODB_WRITER_KEYID read=* write=* publish=*
principal = $GEODB_READER_KEYID read=* write=- publish=-
grant_ttl_s = $GRANT_TTL
BINDING
if [ -n "${GEODB_CURSOR_KEYID:-}" ]; then
	sed -i "/^grant_ttl_s/i principal = $GEODB_CURSOR_KEYID read=- write=- publish=- derived=10" "$RUN/binding.txt"
fi

# Patch the two ids into the ConfigMap (create it if absent). Succeeds quietly where there is no cluster.
publish_ids() {
	[ -n "${KUBERNETES_SERVICE_HOST:-}" ] && [ -r "$SA/token" ] || return 0
	ns="$(cat "$SA/namespace")"
	api="https://$KUBERNETES_SERVICE_HOST:${KUBERNETES_SERVICE_PORT:-443}/api/v1/namespaces/$ns/configmaps"
	data="\"node-id\":\"$1\",\"sealer-id\":\"$2\""
	code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 --cacert "$SA/ca.crt" \
		-H "Authorization: Bearer $(cat "$SA/token")" -H 'Content-Type: application/merge-patch+json' \
		-X PATCH --data "{\"data\":{$data}}" "$api/$CONFIGMAP")"
	[ "$code" = 200 ] && return 0
	if [ "$code" = 404 ]; then
		code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 --cacert "$SA/ca.crt" \
			-H "Authorization: Bearer $(cat "$SA/token")" -H 'Content-Type: application/json' \
			-X POST --data "{\"metadata\":{\"name\":\"$CONFIGMAP\"},\"data\":{$data}}" "$api")"
		[ "$code" = 201 ] && return 0
	fi
	echo "geodb-supervisor: ConfigMap publish failed (HTTP $code), will retry" >&2
	return 1
}

SERVE_PID=
on_term() {
	if [ -n "$SERVE_PID" ] && kill -0 "$SERVE_PID" 2>/dev/null; then
		kill -INT "$SERVE_PID" 2>/dev/null
		wait "$SERVE_PID" 2>/dev/null
	fi
	exit 0
}
trap on_term TERM INT

rm -f "$RUN/geodb.sock" "$CTL/serving"
: > "$RUN/serve.log"
geodb-serve --binding "$RUN/binding.txt" --root "$STORE" --config "$CONFIG" --profile "$PROFILE" \
	--secret-file "$GEODB_STORE_SECRET_FILE" > "$RUN/serve.log" 2>&1 &
SERVE_PID=$!
tail -n +1 -F "$RUN/serve.log" &
TAIL_PID=$!

node=
sealer=
published=0
while kill -0 "$SERVE_PID" 2>/dev/null; do
	if [ -z "$node" ] || [ -z "$sealer" ]; then
		node="$(sed -n 's/^node: \([0-9a-f]\{64\}\).*/\1/p' "$RUN/serve.log" | head -n 1)"
		sealer="$(sed -n 's/^sealer: \([0-9a-f]\{64\}\).*/\1/p' "$RUN/serve.log" | head -n 1)"
		if [ -n "$node" ] && [ -n "$sealer" ]; then
			printf '%s\n' "$node" > "$CTL/node-id"
			printf '%s\n' "$sealer" > "$CTL/sealer-id"
			: > "$CTL/serving"
		fi
	fi
	if [ -n "$node" ] && [ -n "$sealer" ] && [ "$published" = 0 ]; then
		publish_ids "$node" "$sealer" && published=1
	fi
	sleep 0.5
done

wait "$SERVE_PID"
rc=$?
kill "$TAIL_PID" 2>/dev/null
# serve exited unasked: exit so the kubelet restarts the container and readiness drops the pod meanwhile.
echo "geodb-supervisor: geodb-serve exited (rc=$rc)" >&2
exit "$rc"
