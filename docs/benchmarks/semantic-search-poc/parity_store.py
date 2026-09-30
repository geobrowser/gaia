"""Store-level parity: the same query vectors against geo-lens's Neo4j vector index and the PoC
OpenSearch index. Same artifacts, same vectors -> top-10 scores should agree; ids may differ on ties."""
import uuid, statistics, httpx
from neo4j import GraphDatabase
OS="http://localhost:9200"; INDEX="testnet_entities_poc"; VEC="emb_14a3531800c8"; K=10
c=httpx.Client(timeout=120); drv=GraphDatabase.driver("bolt://localhost:7687",auth=("neo4j","lens-dev-password"))
sample=c.post(f"{OS}/{INDEX}/_search",json={"size":20,"_source":["entity_id",VEC],"query":{"function_score":{"query":{"exists":{"field":VEC}},"random_score":{"seed":7,"field":"_seq_no"}}}}).json()["hits"]["hits"]
agree_top=0; score_gaps=[]; set_overlap=[]
with drv.session() as sess:
    for h in sample:
        v=h["_source"][VEC]
        os_hits=c.post(f"{OS}/{INDEX}/_search",json={"size":K,"_source":["entity_id"],"collapse":{"field":"entity_id"},"query":{"knn":{VEC:{"vector":v,"k":50,"method_parameters":{"ef_search":256}}}}}).json()["hits"]["hits"]
        os_top=[(x["_source"]["entity_id"].replace("-",""),round(x["_score"],4)) for x in os_hits][:K]
        # Neo4j: exact rescore of the index's candidates, as geo-lens's vector strategy does
        rows=sess.run("CALL db.index.vector.queryNodes('vec_28644288d488eac0_14a3531800c8', 50, $v) YIELD node, score "
                      "WITH node, vector.similarity.cosine(node.emb_14a3531800c8, $v) AS s RETURN node.id AS id, s ORDER BY s DESC LIMIT $k", v=v, k=K).data()
        nj_top=[(r["id"],round(r["s"],4)) for r in rows]
        agree_top += os_top[0][1]==nj_top[0][1] and os_top[0][0]==nj_top[0][0]
        score_gaps.append(max(abs(a[1]-b[1]) for a,b in zip(os_top,nj_top)))
        set_overlap.append(len({i for i,_ in os_top} & {i for i,_ in nj_top})/K)
print(f"queries={len(sample)} | top-1 identical (id+score): {agree_top}/{len(sample)} | max |Δscore| over top-{K}: {max(score_gaps):.4f} (mean {statistics.mean(score_gaps):.4f}) | mean id-set overlap@{K}: {statistics.mean(set_overlap):.2f} (ties on duplicate claims move ids, not scores)")
