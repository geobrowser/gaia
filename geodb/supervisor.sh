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
# Optional: an HTTP listener so Prometheus can read GET /metrics (geodb_derived_*, geodb_store_*,
# geodb_requests_*, geodb_work_*). Unset = no listener. `routes=metrics,version` (geo-db 5fbf4ad5ee,
# ISS-601-293) makes it answer only GET /metrics and GET /version: every other route is refused 404
# before any session opens. NetworkPolicy geodb-ingress also keeps 9091 to the monitoring namespace.
METRICS_PORT="${GEODB_METRICS_PORT:-}"
GRANT_TTL="${GEODB_GRANT_TTL_S:-300}"
CONFIGMAP="${GEODB_CONFIGMAP:-geodb-space-map}"
SA=/var/run/secrets/kubernetes.io/serviceaccount

# Collections 27, 28 and 29 hold per-person rows and are restricted (`derived_restricted`, geo-db 9b871ed4e4): only a
# principal with write=*, the collection in derived= or in derived_read= may read them. The shared reader key has no
# derived_read=: every indexer mounts it. The api's private reader key (below, optional) is the only reader of 28 and 29.
# The writer may publish (found) any Space; the reader reads and nothing else. The optional cursor key
# (hermes's meta-cursor mirror, Secret geodb-cursor-key) may write derived collection 10 and nothing
# else: no read, no graph write, no publish. Needs geodb-serve at geo-db d903c8e017 or later (`derived=`).
cat > "$RUN/binding.txt" <<BINDING
version = 1
listen = tcp:0.0.0.0:$PORT
principal = $GEODB_WRITER_KEYID read=* write=* publish=*
principal = $GEODB_READER_KEYID read=* write=- publish=-
derived_restricted = 27,28,29
grant_ttl_s = $GRANT_TTL
BINDING
if [ -n "$METRICS_PORT" ]; then
	sed -i "/^grant_ttl_s/i listen = http:0.0.0.0:$METRICS_PORT admits=anonymous routes=metrics,version" "$RUN/binding.txt"
fi
if [ -n "${GEODB_CURSOR_KEYID:-}" ]; then
	sed -i "/^grant_ttl_s/i principal = $GEODB_CURSOR_KEYID read=- write=- publish=- derived=10" "$RUN/binding.txt"
fi
# The notification-indexer's outbox key: the work class (work=*) and nothing else (RUNBOOK section 11).
if [ -n "${GEODB_OUTBOX_KEYID:-}" ]; then
	sed -i "/^grant_ttl_s/i principal = $GEODB_OUTBOX_KEYID read=- write=- publish=- work=*" "$RUN/binding.txt"
fi
# The score writers' key (vote-indexer + ranking-indexer + topology-indexer, Secret geodb-scores-key): derived collections 0-7, 11, 12, 13 to 19, 20 to 29
# (entity_type_ranking, entity_topic_ranking, global/local/space_scores, account_weights, ranks.ranking_scores, account_exclusions, space_topic_suggestions, then the per-user 27 external_interest_signals, 28 user_topic_signals, 29 feed_user_seen_daily) and nothing else (RUNBOOK section 12).
if [ -n "${GEODB_SCORES_KEYID:-}" ]; then
	sed -i "/^grant_ttl_s/i principal = $GEODB_SCORES_KEYID read=- write=- publish=- derived=0,1,2,3,4,5,6,7,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29" "$RUN/binding.txt"
fi

# The api's private reader key (Secret geodb-private-reader-key, k8s/v2/geodb-private-reader-key-job.yaml): reads the
# restricted per-user collections 28 (user_topic_signals) and 29 (feed_user_seen_daily) and nothing else; no Space,
# no write, no publish. Needs geodb-serve at geo-db 9b871ed4e4 or later (`derived_read=`).
if [ -n "${GEODB_PRIVATE_READER_KEYID:-}" ]; then
	sed -i "/^grant_ttl_s/i principal = $GEODB_PRIVATE_READER_KEYID read=- write=- publish=- derived_read=28,29" "$RUN/binding.txt"
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
