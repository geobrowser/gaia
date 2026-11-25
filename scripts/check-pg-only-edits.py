#!/usr/bin/env python3
"""Find edits that reached Postgres without a geo-db twin (read only).

`kg-indexer replay_edit` writes Postgres only; the geo-db side is the `geodb-replay-edit` Job
(geodb/RUNBOOK.md section 2b-quater). When the Job is skipped, geo-db never holds the edit's entities
(edit 6a1398bd, version key 125690:0, Space 88883e1e: 45 entities in Postgres, 4 in geo-db).

`replay_edit` writes sequence 0 unless told otherwise, but a live edit that is first in its block also has
sequence 0, so sequence 0 only names candidates. The test is whether geo-db holds the edit's entities.

Steps (nothing here writes, and nothing needs a credential this script handles):

  1. list     seq-0 edits since a cutoff, from the public GraphQL (cheap, paged).
       check-pg-only-edits.py list --since 2026-11-05 > edits.tsv
  2. sql      the query a Postgres reader runs (psql in an api pod) for those edits' (edit, space, entity) pairs.
              It filters value_versions and relation_versions on valid_from_key, which has no index of its own,
              so it is NOT sent through the public GraphQL (that scan times out).
       check-pg-only-edits.py sql edits.tsv > pairs.sql   # psql -At -F'|' -f pairs.sql > pairs.txt
  3. check    compare against what the feeder journalled: /data/feeder/known-entities.log (lines
              "<geo space>|<entity>") and the published space-map.json ({gaia space: geo space}). Read both
              with `cat`; a Space missing from the map is reported, not skipped.
       check-pg-only-edits.py check --pairs pairs.txt --known known-entities.log \\
           --space-map space-map.json --edits edits.tsv [--cids cids.tsv]

The report names Space, edit id, block, how many entities are missing, and the `geodb-replay-edit`
arguments. The CID is not in Postgres edit_versions: pass --cids (lines "<edit id>\\t<ipfs://cid>"), else the
report says `<cid>`.
"""
import argparse
import calendar
import hashlib
import json
import re
import sys
import time
import urllib.request

GRAPHQL = "https://api-testnet.geobrowser.io/graphql"
UUID_V4 = re.compile(r"^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$")


def compact(i):
    return i.replace("-", "").lower()


def conforming_id(kind, legacy_id):
    """api/src/geodb/card.ts conformingId: a v4 id is itself, any other is a sha256 remap."""
    i = compact(legacy_id)
    if UUID_V4.match(i):
        return i
    b = bytearray(bytes.fromhex(hashlib.sha256(f"geo:legacy-{kind}:v1\0{i}".encode()).hexdigest()[:32]))
    b[6] = (b[6] & 0x0F) | 0x40
    b[8] = (b[8] & 0x3F) | 0x80
    return b.hex()


def version_key(block, sequence):
    return (int(block) << 32) | int(sequence)


def unix_seconds(created_at):
    return calendar.timegm(time.strptime(created_at[:19], "%Y-%m-%dT%H:%M:%S"))


def parse_pairs(lines):
    out = []
    for n, line in enumerate(lines, 1):
        line = line.strip()
        if not line:
            continue
        parts = line.split("|")
        if len(parts) != 3:
            raise ValueError(f"pairs line {n}: want edit|space|entity, got {line!r}")
        out.append((compact(parts[0]), compact(parts[1]), compact(parts[2])))
    return out


def missing_by_edit(pairs, known, space_map):
    """One row per (edit, space) with at least one entity geo-db does not hold."""
    groups = {}
    for edit, space, entity in pairs:
        groups.setdefault((edit, space), set()).add(entity)
    rows = []
    for (edit, space), entities in sorted(groups.items()):
        geo_space = space_map.get(space)
        if geo_space is None:
            rows.append({"edit": edit, "space": space, "missing": len(entities), "total": len(entities), "unmapped_space": True})
            continue
        gone = [e for e in entities if f"{geo_space}|{conforming_id('entity', e)}" not in known]
        if gone:
            rows.append({"edit": edit, "space": space, "missing": len(gone), "total": len(entities), "unmapped_space": False})
    return rows


def render(rows, edits):
    out = ["space\tedit\tblock\tmissing/total\treplay arguments"]
    for r in rows:
        e = edits.get(r["edit"], {})
        block, seq = e.get("block"), e.get("sequence", 0)
        at = unix_seconds(e["createdAt"]) if e.get("createdAt") else "<unix>"
        note = " (space not in space-map: geo-db has no such Space)" if r["unmapped_space"] else ""
        args = (
            f"--uri {e.get('cid') or '<cid>'} --space {r['space']} --block {block} "
            f"--sequence {seq} --created-at {at} --edit {r['edit']}"
        )
        out.append(f"{r['space']}\t{r['edit']}\t{block}\t{r['missing']}/{r['total']}{note}\t{args}")
    return "\n".join(out)


def gql(query):
    req = urllib.request.Request(
        GRAPHQL, json.dumps({"query": query}).encode(), {"content-type": "application/json", "user-agent": "check-pg-only-edits"}
    )
    with urllib.request.urlopen(req, timeout=90) as resp:
        body = json.load(resp)
    if body.get("errors"):
        raise RuntimeError(body["errors"])
    return body["data"]


def list_edits(since):
    """Sequence-0 edits created at or after `since` (YYYY-MM-DD), oldest first."""
    after, out = None, []
    while True:
        cur = f', after: "{after}"' if after else ""
        q = (
            "{ editVersionsConnection(first: 200, orderBy: BLOCK_NUMBER_ASC" + cur
            + f', filter: {{sequence: {{is: "0"}}, createdAt: {{greaterThanOrEqualTo: "{since}T00:00:00"}}}}) '
            "{ pageInfo { hasNextPage endCursor } nodes { editId blockNumber sequence versionKey createdAt name } } }"
        )
        page = gql(q)["editVersionsConnection"]
        out += page["nodes"]
        if not page["pageInfo"]["hasNextPage"]:
            return out
        after = page["pageInfo"]["endCursor"]


def read_edits(path):
    edits = {}
    for line in open(path):
        f = line.rstrip("\n").split("\t")
        if len(f) >= 4 and f[0] != "editId":
            edits[compact(f[0])] = {"block": int(f[1]), "sequence": int(f[2]), "createdAt": f[3]}
    return edits


def pairs_sql(edits):
    keys = ", ".join(str(version_key(e["block"], e["sequence"])) for e in edits.values())
    return (
        "-- read only: (edit, space, entity) for each listed edit's version key\n"
        "SELECT ev.edit_id, x.space_id, x.entity_id FROM edit_versions ev JOIN (\n"
        f"  SELECT valid_from_key AS k, space_id, entity_id FROM value_versions WHERE valid_from_key IN ({keys})\n"
        f"  UNION SELECT valid_from_key, space_id, entity_id FROM relation_versions WHERE valid_from_key IN ({keys})\n"
        ") x ON x.k = ev.version_key GROUP BY 1, 2, 3 ORDER BY 1, 2, 3;"
    )


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list")
    p.add_argument("--since", required=True)
    p = sub.add_parser("sql")
    p.add_argument("edits")
    p = sub.add_parser("check")
    for flag in ("--pairs", "--known", "--space-map", "--edits"):
        p.add_argument(flag, required=True)
    p.add_argument("--cids")
    a = ap.parse_args(argv)
    if a.cmd == "list":
        print("editId\tblockNumber\tsequence\tcreatedAt\tname")
        for n in list_edits(a.since):
            print("\t".join([n["editId"], n["blockNumber"], n["sequence"], n["createdAt"], (n["name"] or "").replace("\t", " ")]))
    elif a.cmd == "sql":
        print(pairs_sql(read_edits(a.edits)))
    else:
        edits = read_edits(a.edits)
        if a.cids:
            for line in open(a.cids):
                f = line.split()
                if len(f) == 2 and compact(f[0]) in edits:
                    edits[compact(f[0])]["cid"] = f[1]
        known = {l.strip() for l in open(a.known) if l.strip()}
        space_map = {compact(k): compact(v) for k, v in json.load(open(a.space_map)).items()}
        rows = missing_by_edit(parse_pairs(open(a.pairs)), known, space_map)
        print(render(rows, edits))
        return 1 if rows else 0
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
