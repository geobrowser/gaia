#!/usr/bin/env bash
# Offline unit tests for geodb/supervisor.sh flag handling and lifecycle.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
fail=0
check() {
	if [[ "$2" == "$3" ]]; then
		echo "ok   $1"
	else
		echo "FAIL $1"
		echo "  want: $3"
		echo "  got:  $2"
		fail=1
	fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

run_case() {
	local name="$1"
	local help_mode="$2"
	local metrics_port="${3:-}"
	local case_dir="$tmp/$name"
	local bin_dir="$case_dir/bin"
	local data_dir="$case_dir/data"
	local args_file="$case_dir/args.txt"
	local secret_file="$case_dir/secret.hex"
	local cfg_file="$case_dir/serve.cfg"

	mkdir -p "$bin_dir" "$data_dir"
	printf '%064d\n' 0 >"$secret_file"
	printf 'version = 1\n' >"$cfg_file"

	# Use python3 so the mock installs a real SIGINT handler (overriding POSIX sh's
	# inherited SIG_IGN for background jobs, matching the real geodb-serve binary).
	cat >"$bin_dir/geodb-serve" <<MOCK
#!/usr/bin/env python3
import signal, sys, time

if len(sys.argv) > 1 and sys.argv[1] == "--help":
    mode = "$help_mode"
    if mode == "with-flag":
        print("Options:")
        print("  --allow-plaintext     Permit non-loopback listeners without confidential")
        sys.exit(0)
    elif mode == "without-flag":
        print("Options:")
        print("  --binding <path>      The binding file")
        sys.exit(0)
    else:
        print("unknown flag --help", file=sys.stderr)
        sys.exit(1)

with open("$args_file", "w", encoding="utf-8") as f:
    for arg in sys.argv[1:]:
        f.write(arg + "\n")

def handle_sig(signum, frame):
    sys.exit(0)

signal.signal(signal.SIGINT, handle_sig)
signal.signal(signal.SIGTERM, handle_sig)

print("node: 1111111111111111111111111111111111111111111111111111111111111111", flush=True)
print("sealer: 2222222222222222222222222222222222222222222222222222222222222222", flush=True)
while True:
    time.sleep(0.1)
MOCK
	chmod +x "$bin_dir/geodb-serve"

	local sup_pid=""
	PATH="$bin_dir:$PATH" \
	DATA_DIR="$data_dir" \
	GEODB_WRITER_KEYID="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" \
	GEODB_READER_KEYID="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" \
	GEODB_STORE_SECRET_FILE="$secret_file" \
	GEODB_CONFIG="$cfg_file" \
	GEODB_PROFILE="tiny" \
	GEODB_PORT="17447" \
	GEODB_METRICS_PORT="$metrics_port" \
	KUBERNETES_SERVICE_HOST="" \
		sh "$here/supervisor.sh" >"$case_dir/supervisor.out" 2>"$case_dir/supervisor.err" &
	sup_pid=$!

	local ready=0
	for _ in $(seq 1 50); do
		if [[ -e "$data_dir/ctl/serving" ]]; then
			ready=1
			break
		fi
		if ! kill -0 "$sup_pid" 2>/dev/null; then
			break
		fi
		sleep 0.1
	done

	kill -TERM "$sup_pid" 2>/dev/null || true
	local exited=0
	for _ in $(seq 1 50); do
		if ! kill -0 "$sup_pid" 2>/dev/null; then
			exited=1
			break
		fi
		sleep 0.1
	done
	if [[ "$exited" -eq 0 ]]; then
		kill -KILL "$sup_pid" 2>/dev/null || true
	fi
	local rc=0
	wait "$sup_pid" || rc=$?

	check "$name: reaches ready" "$ready" "1"
	check "$name: clean SIGTERM exit" "$rc" "0"
	check "$name: node-id written" "$(cat "$data_dir/ctl/node-id" 2>/dev/null || true)" \
		"1111111111111111111111111111111111111111111111111111111111111111"
	check "$name: sealer-id written" "$(cat "$data_dir/ctl/sealer-id" 2>/dev/null || true)" \
		"2222222222222222222222222222222222222222222222222222222222222222"
	check "$name: binding tcp listen" "$(grep -c '^listen = tcp:0\.0\.0\.0:17447$' "$data_dir/run/binding.txt" || true)" "1"
	if [[ -n "$metrics_port" ]]; then
		check "$name: binding http metrics listen" \
			"$(grep -c "^listen = http:0\\.0\\.0\\.0:${metrics_port} admits=anonymous routes=metrics,version$" "$data_dir/run/binding.txt" || true)" "1"
	fi
}

# 1. geodb-serve advertising --allow-plaintext receives the flag
run_case "with-flag" "with-flag"
check "with-flag: passes --allow-plaintext" \
	"$(grep -cx -- '--allow-plaintext' "$tmp/with-flag/args.txt" || true)" "1"

# 2. geodb-serve without --allow-plaintext (e.g. 99fdac58cf) does NOT receive it
run_case "without-flag" "without-flag"
check "without-flag: omits --allow-plaintext" \
	"$(grep -cx -- '--allow-plaintext' "$tmp/without-flag/args.txt" || true)" "0"

# 3. geodb-serve whose --help fails does NOT receive --allow-plaintext
run_case "help-fails" "help-fails"
check "help-fails: omits --allow-plaintext" \
	"$(grep -cx -- '--allow-plaintext' "$tmp/help-fails/args.txt" || true)" "0"

# 4. With GNU sed (Linux container), verify metrics HTTP listen is also emitted
if sed --version >/dev/null 2>&1; then
	run_case "with-metrics" "with-flag" "19091"
	check "with-metrics: passes --allow-plaintext" \
		"$(grep -cx -- '--allow-plaintext' "$tmp/with-metrics/args.txt" || true)" "1"
fi

exit "$fail"
