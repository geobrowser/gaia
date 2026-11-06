#!/bin/sh
# Fail when a tracked symlink points outside the repository or at nothing. Such a link is almost always a git worktree
# pointer (../../gaia-w-xyz) swept up by `git add -A`; it is meaningless on any other checkout.
set -eu
bad=0
git ls-files -s | awk '$1 == "120000" {print $4}' | while IFS= read -r f; do
	target=$(git show ":$f")
	dir=$(dirname "$f")
	# Normalise dir/target without touching the filesystem; a leading .. that climbs past the root escapes.
	resolved=$(printf '%s/%s\n' "$dir" "$target" | awk -F/ '{n=0; for(i=1;i<=NF;i++){ if($i==""||$i==".")continue; if($i==".."){ if(n==0){print "ESCAPES"; exit} n--; } else {p[++n]=$i} } if(n>=0)print "ok"}')
	if [ "$resolved" = "ESCAPES" ]; then echo "tracked symlink escapes the repository: $f -> $target"; echo bad >> "${TMPDIR:-/tmp}/symlink-guard.$$"; fi
done
if [ -f "${TMPDIR:-/tmp}/symlink-guard.$$" ]; then rm -f "${TMPDIR:-/tmp}/symlink-guard.$$"; exit 1; fi
