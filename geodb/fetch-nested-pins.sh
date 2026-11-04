#!/bin/sh
# Fetch the first-party packages that geo-db's siblings pin in turn, into Zig's package cache.
#
# geo-db's zig/siblings.sh fetches each package geo-db itself pins. A sibling can pin a different
# commit of another sibling (envelope pins zblake3 53563ac2, geo-db pins 94799260), and Zig's own
# fetcher cannot authenticate to these private repositories, so `zig build` fails with
# "unable to discover remote git server capabilities". This walks every build.zig.zon in the
# checkout and in the cached packages, clones each first-party pin not yet cached under git's
# credentials, and `zig fetch`es it, checking the hash, until a pass fetches nothing new.
#
# Run from the geo-db checkout, after zig/siblings.sh, with git configured to authenticate to
# github.com/geobrowser.
set -eu
cache=${ZIG_GLOBAL_CACHE_DIR:-$HOME/.cache/zig}
pass=0
while :; do
    pass=$((pass + 1))
    [ "$pass" -le 8 ] || { echo "nested pins did not settle after 8 passes" >&2; exit 1; }
    work=$(mktemp -d)
    for t in "$cache"/p/*.tar.gz; do
        [ -e "$t" ] || continue
        d="$work/$(basename "$t" .tar.gz)"
        mkdir -p "$d"
        tar -xzf "$t" -C "$d"
    done
    pins=$(find . "$work" -name build.zig.zon -not -path './zig-pkg/*' -not -path './.zig-cache/*' -exec cat {} + |
        python3 -c '
import re, sys
for url, commit, h in sorted(set(re.findall(
        r"\.url\s*=\s*\"git\+(https://github\.com/geobrowser/[^\"#]+)#([0-9a-f]{40})\"\s*,\s*\.hash\s*=\s*\"([^\"]+)\"",
        sys.stdin.read()))):
    print(url, commit, h)
')
    rm -rf "$work"
    fetched=0
    echo "$pins" | while read -r url commit hash; do
        [ -n "$url" ] || continue
        if [ -e "$cache/p/$hash" ] || [ -e "$cache/p/$hash.tar.gz" ]; then continue; fi
        src=$(mktemp -d)
        git init -q "$src"
        git -C "$src" fetch -q --depth 1 "$url" "$commit"
        git -C "$src" checkout -q --detach FETCH_HEAD
        got=$(zig fetch "$src")
        rm -rf "$src"
        if [ "$got" != "$hash" ]; then
            echo "$url#$commit fetched as $got, not the pinned $hash" >&2; exit 1
        fi
        echo "$url $commit $hash fetched"
    done > "${TMPDIR:-/tmp}/nested-pins.$$"
    fetched=$(wc -l < "${TMPDIR:-/tmp}/nested-pins.$$")
    cat "${TMPDIR:-/tmp}/nested-pins.$$"
    rm -f "${TMPDIR:-/tmp}/nested-pins.$$"
    [ "$fetched" -gt 0 ] || break
done
echo "nested pins settled after $pass passes"
