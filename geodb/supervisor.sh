#!/bin/sh
# Runs geodb-serve for the geodb StatefulSet and restarts it when the feeder asks.
#
# Why a supervisor: geo-db founds a Space only at serve startup (--found <nonce>), so the feeder has to
# restart serve to add one. The feeder cannot do that itself (separate container), so it drops control
# files on the shared volume and this script acts on them:
#
#   $DATA/ctl/pending-found   a founding nonce (hex) for the next start; consumed (deleted) on use
#   $DATA/ctl/restart         present = stop serve cleanly (SIGINT, final checkpoint) and start it again
#   $DATA/ctl/node-id         written here: serve's node key id, which clients pin in their binding
#
# serve and the feeder talk over a Unix socket in $DATA/run (relative to the cwd), so both run there.
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

# The binding is derived from the environment on every start, so a rotated key id takes effect on restart.
cat > "$RUN/binding.txt" <<BINDING
version = 1
listen = unix:./geodb.sock
listen = tcp:0.0.0.0:$PORT
principal = $GEODB_WRITER_KEYID read=* write=* publish=*
principal = $GEODB_READER_KEYID read=* write=- publish=-
grant_ttl_s = $GRANT_TTL
BINDING

SERVE_PID=
STOPPING=0

stop_serve() {
	if [ -n "$SERVE_PID" ] && kill -0 "$SERVE_PID" 2>/dev/null; then
		kill -INT "$SERVE_PID" 2>/dev/null
		wait "$SERVE_PID" 2>/dev/null
	fi
}

on_term() {
	STOPPING=1
	stop_serve
	exit 0
}
trap on_term TERM INT

while :; do
	found=
	if [ -s "$CTL/pending-found" ]; then
		found="$(cat "$CTL/pending-found")"
		rm -f "$CTL/pending-found"
	fi
	rm -f "$CTL/restart" "$RUN/geodb.sock" "$CTL/serving"

	: > "$RUN/serve.log"
	if [ -n "$found" ]; then
		geodb-serve --binding "$RUN/binding.txt" --root "$STORE" --config "$CONFIG" --profile "$PROFILE" \
			--secret-file "$GEODB_STORE_SECRET_FILE" --found "$found" > "$RUN/serve.log" 2>&1 &
	else
		geodb-serve --binding "$RUN/binding.txt" --root "$STORE" --config "$CONFIG" --profile "$PROFILE" \
			--secret-file "$GEODB_STORE_SECRET_FILE" > "$RUN/serve.log" 2>&1 &
	fi
	SERVE_PID=$!
	tail -n +1 -F "$RUN/serve.log" &
	TAIL_PID=$!

	node=
	restart=0
	while kill -0 "$SERVE_PID" 2>/dev/null; do
		if [ -z "$node" ]; then
			node="$(sed -n 's/^node: \([0-9a-f]\{64\}\).*/\1/p' "$RUN/serve.log" | head -n 1)"
			if [ -n "$node" ]; then
				printf '%s\n' "$node" > "$CTL/node-id.tmp" && mv "$CTL/node-id.tmp" "$CTL/node-id"
				: > "$CTL/serving"
			fi
		fi
		if [ -e "$CTL/restart" ]; then
			restart=1
			stop_serve
			break
		fi
		sleep 0.5
	done

	if [ "$restart" = 0 ]; then
		wait "$SERVE_PID"
		rc=$?
		kill "$TAIL_PID" 2>/dev/null
		# Not asked to restart: serve died. Exit so the kubelet restarts the container and the
		# readiness probe drops the pod from the Service meanwhile.
		echo "geodb-supervisor: geodb-serve exited unrequested (rc=$rc)" >&2
		exit "$rc"
	fi
	kill "$TAIL_PID" 2>/dev/null
	echo "geodb-supervisor: restarting geodb-serve at the feeder's request" >&2
done
