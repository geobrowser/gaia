"""Step 0 PoC benchmark against testnet_entities_poc: each query shape from the design vs a
brute-force script_score reference. Usage: bench.py [n_queries] [--forcemerge]"""
import json, random, statistics, sys, time, uuid
import httpx

OS = "http://localhost:9200"; INDEX = "testnet_entities_poc"; SLOT = "14a3531800c8"; VEC = f"emb_{SLOT}"
BIG = str(uuid.UUID("b5a31f8182b042437ede0f84ee02f104"))    # 192k members
SMALL = str(uuid.UUID("4582fbbee28a16589154f7e36f1ee3c5"))  # 11k members
DEBATE_TAG = str(uuid.UUID("55c95b2626f8482cb9739ea99dfde438"))  # 475 claims
N = int(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1].isdigit() else 60
c = httpx.Client(timeout=300)

if "--forcemerge" in sys.argv:
    t = time.time(); c.post(f"{OS}/{INDEX}/_forcemerge?max_num_segments=1"); print(f"forcemerge: {time.time()-t:.0f}s")

def search(body, params=""):
    t = time.perf_counter(); r = c.post(f"{OS}/{INDEX}/_search{params}", json=body).json(); dt = (time.perf_counter() - t) * 1000
    if "error" in r: raise RuntimeError(r["error"]["root_cause"][0]["reason"])
    return [(h["_id"], round(h["_score"], 5)) for h in r["hits"]["hits"]], dt, r.get("took")

def pct(xs, p): xs = sorted(xs); return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]
def flt(space=None, tag=None):
    f = []
    if space: f.append({"term": {"space_id": space}})
    if tag: f.append({"nested": {"path": "relations", "query": {"term": {"relations.to_entity_id": tag}}}})
    return {"bool": {"filter": f}} if f else None
def knn(vec, k=None, min_score=None, filt=None, size=10, frm=0, ef=None):
    q = {"vector": vec}
    if ef is not None: q["method_parameters"] = {"ef_search": ef}
    if k is not None: q["k"] = k
    if min_score is not None: q["min_score"] = min_score
    if filt: q["filter"] = filt
    return {"size": size, "from": frm, "_source": False, "query": {"knn": {VEC: q}}}
def brute(vec, filt=None, size=10, min_score=None):
    body = {"size": size, "_source": False, "query": {"script_score": {"query": filt or {"match_all": {}},
            "script": {"source": f"(1 + cosineSimilarity(params.q, doc['{VEC}'])) / 2", "params": {"q": vec}}}}}
    if min_score is not None: body["min_score"] = min_score
    return body

# Sample query docs: random members of the big space, with their vectors and names.
sample = c.post(f"{OS}/{INDEX}/_search", json={"size": N, "_source": ["name", "space_id", VEC],
    "query": {"function_score": {"query": {"bool": {"filter": [{"exists": {"field": VEC}}, {"term": {"space_id": BIG}}]}},
              "random_score": {"seed": 42, "field": "_seq_no"}}}}).json()["hits"]["hits"]
queries = [(h["_id"], h["_source"]["name"], h["_source"][VEC]) for h in sample]
info = c.get(f"{OS}/_cat/indices/{INDEX}?h=docs.count,store.size,segments.count&bytes=mb").text.split()
ef_setting = c.get(f"{OS}/{INDEX}/_settings?include_defaults=true&filter_path=**.knn*").json()
print(f"index: docs={info[0]} store={info[1]}MB segments={info[2]} | queries={len(queries)} (random members of the 192k space) | knn settings: {json.dumps(ef_setting)[:200]}\n")

rows = []
def run(name, mk_query, mk_ref=None, note=""):
    lat, took, recall, hit1 = [], [], [], 0
    for qid, qname, vec in queries:
        got, dt, tk = search(*mk_query(vec, qname)); lat.append(dt); took.append(tk or 0)
        if got and got[0][0] == qid: hit1 += 1
        if mk_ref:
            ref, _, _ = search(*mk_ref(vec, qname))
            g = {i for i, _ in got}; r = {i for i, _ in ref}
            id_rec = len(g & r) / len(r) if r else 1.0
            kth = ref[-1][1] if ref else 0.0
            sc_rec = sum(1 for _, sc in got if sc >= kth - 1e-5) / len(ref) if ref else 1.0
            recall.append((id_rec, sc_rec))
    rows.append((name, f"{pct(lat,50):.0f}", f"{pct(lat,95):.0f}", f"{pct(took,95):.0f}",
                 (f"{statistics.mean(x[0] for x in recall):.3f} / {statistics.mean(x[1] for x in recall):.3f}") if recall else "—", f"{hit1}/{len(queries)}", note))
    print(f"  done: {name}", flush=True)

run("knn k=10, no filter", lambda v, n: (knn(v, k=10),), lambda v, n: (brute(v),), "HNSW recall vs brute force over all docs")
run("brute force, no filter (reference cost)", lambda v, n: (brute(v),))
run("knn k=10, filter space 192k", lambda v, n: (knn(v, k=10, filt=flt(space=BIG)),), lambda v, n: (brute(v, flt(space=BIG)),), "filter inside the clause")
run("knn k=10, no filter, ef_search=100", lambda v, n: (knn(v, k=10, ef=100),), lambda v, n: (brute(v),), "per-request method_parameters")
run("knn k=10, no filter, ef_search=256", lambda v, n: (knn(v, k=10, ef=256),), lambda v, n: (brute(v),))
run("knn k=10, filter space 192k, ef_search=256", lambda v, n: (knn(v, k=10, filt=flt(space=BIG), ef=256),), lambda v, n: (brute(v, flt(space=BIG)),))
run("brute force, space 192k (reference cost)", lambda v, n: (brute(v, flt(space=BIG)),))
run("knn k=10, filter space 11k", lambda v, n: (knn(v, k=10, filt=flt(space=SMALL)),), lambda v, n: (brute(v, flt(space=SMALL)),))
run("knn k=10, filter Debate tag (475)", lambda v, n: (knn(v, k=10, filt=flt(tag=DEBATE_TAG)),), lambda v, n: (brute(v, flt(tag=DEBATE_TAG)),), "nested relation filter; exact fallback expected")
run("radial min_score 0.85, space 192k, size 50", lambda v, n: (knn(v, min_score=0.85, filt=flt(space=BIG), size=50),), lambda v, n: (brute(v, flt(space=BIG), size=50, min_score=0.85),), "semantic mode shape")
run("radial min_score 0.85, space 192k, from 10 size 10", lambda v, n: (knn(v, min_score=0.85, filt=flt(space=BIG), size=10, frm=10),), None, "paging works?")
c.put(f"{OS}/_search/pipeline/poc_hybrid", json={"phase_results_processors": [{"normalization-processor": {"normalization": {"technique": "min_max"}, "combination": {"technique": "arithmetic_mean"}}}]})
run("hybrid: match(name) + knn k=100, space 192k", lambda v, n: ({"size": 10, "_source": False, "query": {"hybrid": {"queries": [
        {"bool": {"must": [{"match": {"name": n}}], "filter": [{"term": {"space_id": BIG}}]}},
        {"knn": {VEC: {"vector": v, "k": 100, "filter": flt(space=BIG)}}}]}}}, "?search_pipeline=poc_hybrid"), None, "query doc should rank first")

print("\n| shape | p50 ms | p95 ms | took p95 | recall@k id-set / score-based | self top-1 | note |\n|---|---|---|---|---|---|---|")
for r in rows: print("| " + " | ".join(r) + " |")
stats = c.get(f"{OS}/_plugins/_knn/stats").json()
node = next(iter(stats["nodes"].values()))
print(f"\nknn stats: graph_memory_usage={node.get('graph_memory_usage')} KB, graph_index_requests={node.get('graph_index_requests')}, hit_count={node.get('hit_count')}, miss_count={node.get('miss_count')}")
