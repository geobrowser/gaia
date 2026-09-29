"""Radial recall with ef_search, vs k-mode + client-side floor (the semantic-mode decision)."""
import json, statistics, sys, time, uuid, httpx
OS="http://localhost:9200"; INDEX="testnet_entities_poc"; VEC="emb_14a3531800c8"
BIG=str(uuid.UUID("b5a31f8182b042437ede0f84ee02f104")); c=httpx.Client(timeout=300)
FL={"bool":{"filter":[{"term":{"space_id":BIG}}]}}
def s(body):
    t=time.perf_counter(); r=c.post(f"{OS}/{INDEX}/_search",json=body).json(); dt=(time.perf_counter()-t)*1000
    if "error" in r: raise RuntimeError(r["error"]["root_cause"][0]["reason"])
    return [(h["_id"],h["_score"]) for h in r["hits"]["hits"]], dt
def pct(xs,p): xs=sorted(xs); return xs[min(len(xs)-1,int(round(p/100*(len(xs)-1))))]
sample=c.post(f"{OS}/{INDEX}/_search",json={"size":60,"_source":[VEC],"query":{"function_score":{"query":{"bool":{"filter":[{"exists":{"field":VEC}},{"term":{"space_id":BIG}}]}},"random_score":{"seed":42,"field":"_seq_no"}}}}).json()["hits"]["hits"]
qs=[h["_source"][VEC] for h in sample]
FLOOR=0.85; SIZE=50
def brute(v): return {"size":SIZE,"_source":False,"min_score":FLOOR,"query":{"script_score":{"query":FL,"script":{"source":f"(1 + cosineSimilarity(params.q, doc['{VEC}'])) / 2","params":{"q":v}}}}}
shapes={
 "radial min_score, no ef":            lambda v: {"size":SIZE,"_source":False,"query":{"knn":{VEC:{"vector":v,"min_score":FLOOR,"filter":FL}}}},
 "k=50 no ef, floor client-side":      lambda v: {"size":SIZE,"_source":False,"query":{"knn":{VEC:{"vector":v,"k":SIZE,"filter":FL}}}},
 "k=50 ef=256, floor client-side":     lambda v: {"size":SIZE,"_source":False,"query":{"knn":{VEC:{"vector":v,"k":SIZE,"filter":FL,"method_parameters":{"ef_search":256}}}}},
 "k=100 ef=256, floor client-side":    lambda v: {"size":SIZE,"_source":False,"query":{"knn":{VEC:{"vector":v,"k":100,"filter":FL,"method_parameters":{"ef_search":256}}}}},
 "k=50 ef=512, floor client-side":     lambda v: {"size":SIZE,"_source":False,"query":{"knn":{VEC:{"vector":v,"k":SIZE,"filter":FL,"method_parameters":{"ef_search":512}}}}},
 "k=10 ef=256, floor client-side (size 10)": lambda v: {"size":10,"_source":False,"query":{"knn":{VEC:{"vector":v,"k":10,"filter":FL,"method_parameters":{"ef_search":256}}}}},
}
print(f"floor={FLOOR} size={SIZE} filter=space 192k, {len(qs)} queries; recall = |hits above floor found| / |brute-force hits above floor| (score-based, capped at size)\n")
print("| shape | p50 ms | p95 ms | recall vs brute | brute avg hits above floor |\n|---|---|---|---|---|")
for name,mk in shapes.items():
    lat=[]; rec=[]; bn=[]
    for v in qs:
        ref,_=s(brute(v)); got,dt=s(mk(v)); lat.append(dt)
        got=[(i,sc) for i,sc in got if sc>=FLOOR-1e-6]     # client-side floor (no-op for radial)
        if "size 10" in name: ref=ref[:10]
        if ref:
            kth=ref[-1][1]; rec.append(min(1.0, sum(1 for _,sc in got if sc>=kth-1e-5)/len(ref)))
        bn.append(len(ref))
    print(f"| {name} | {pct(lat,50):.0f} | {pct(lat,95):.0f} | {statistics.mean(rec):.3f} | {statistics.mean(bn):.1f} |")
