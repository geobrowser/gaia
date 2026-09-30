"""Step 0 PoC loader: geo-lens claims (with their bge-small vectors) -> local OpenSearch, one document
per (entity, space) in gaia's search-document shape, into testnet_entities_poc."""
import json, sys, time, uuid
from datetime import datetime, timezone
import httpx
from neo4j import GraphDatabase

OS = "http://localhost:9200"; INDEX = "testnet_entities_poc"; SLOT = "14a3531800c8"
LABEL = "C_28644288d488eac0"
TYPES = "8f151ba4de204e3c9cb499ddf96f48f1"; TAGS = "257090341ba5406f94e4d4af90042fba"
def dashed(h): return str(uuid.UUID(h)) if isinstance(h, str) and len(h) == 32 else h
now = datetime.now(timezone.utc).isoformat(timespec="seconds")

drv = GraphDatabase.driver("bolt://localhost:7687", auth=("neo4j", "lens-dev-password"))
client = httpx.Client(timeout=120)
t0 = time.time()

# Pass 1: Types/Tags relations, streamed (small rows), grouped in Python.
rels = {}
with drv.session() as s:
    for rec in s.run(f"MATCH (e:Entity:{LABEL})-[r:REL]->(t:Entity) WHERE r.typeId IN [$a, $b] "
                     "RETURN e.id AS eid, r.id AS rid, r.typeId AS tid, t.id AS to", a=TYPES, b=TAGS):
        rels.setdefault(rec["eid"], []).append({"relation_id": dashed(rec["rid"]), "relation_type": dashed(rec["tid"]), "to_entity_id": dashed(rec["to"])})
print(f"relations: {sum(len(v) for v in rels.values())} on {len(rels)} entities ({time.time()-t0:.0f}s)", flush=True)

# Pass 2: entities with vectors, streamed; expand per space; bulk in chunks.
def flush(lines, stats):
    if not lines: return
    r = client.post(f"{OS}/_bulk", content="\n".join(lines) + "\n", headers={"content-type": "application/x-ndjson"})
    body = r.json()
    stats["docs"] += len(lines) // 2
    if body.get("errors"):
        errs = [i["index"] for i in body["items"] if i["index"].get("error")]
        stats["errors"] += len(errs)
        if stats["errors"] <= 3: print("bulk error sample:", json.dumps(errs[0])[:300], flush=True)
    lines.clear()

stats = {"entities": 0, "docs": 0, "errors": 0}
lines = []
with drv.session() as s:
    q = (f"MATCH (e:Entity:{LABEL}) WHERE e.emb_{SLOT} IS NOT NULL "
         f"RETURN e.id AS id, e.name AS name, e.description AS description, e.spaceIds AS spaceIds, "
         f"e.emb_{SLOT} AS vec, e.embhash_{SLOT} AS hash")
    for rec in s.run(q):
        stats["entities"] += 1
        eid = rec["id"]; vec = [round(float(x), 6) for x in rec["vec"]]
        base = {"entity_id": dashed(eid), "name": rec["name"], "name_raw": rec["name"], "indexed_at": now,
                "in_canonical_graph": True, "relations": rels.get(eid, []),
                f"emb_{SLOT}": vec, f"emb_{SLOT}_src_hash": rec["hash"], f"emb_{SLOT}_at": now}
        if rec["description"]: base["description"] = rec["description"]
        for sp in (rec["spaceIds"] or []):
            doc = dict(base); doc["space_id"] = dashed(sp)
            lines.append(json.dumps({"index": {"_index": INDEX, "_id": f"{dashed(eid)}_{dashed(sp)}"}}))
            lines.append(json.dumps(doc, separators=(",", ":")))
        if len(lines) >= 2000: flush(lines, stats)
        if stats["entities"] % 25000 == 0: print(f"  {stats['entities']} entities, {stats['docs']} docs, {stats['errors']} errors, {time.time()-t0:.0f}s", flush=True)
flush(lines, stats)
client.put(f"{OS}/{INDEX}/_settings", json={"index": {"refresh_interval": "1s"}})
client.post(f"{OS}/{INDEX}/_refresh")
cnt = client.get(f"{OS}/{INDEX}/_count").json()["count"]
size = client.get(f"{OS}/_cat/indices/{INDEX}?h=store.size&bytes=mb").text.strip()
print(f"DONE entities={stats['entities']} docs={stats['docs']} errors={stats['errors']} indexed_count={cnt} store={size}MB total={time.time()-t0:.0f}s", flush=True)
