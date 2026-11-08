#!/usr/bin/env python3
"""Golden grid for the 17 Entity computed-column functions (plan 10.22 section 8.4), from a scratch postgres:16.

Loads the LATEST definition of each `public.entities_*` column function (api/drizzle) next to the trending graph the group B grid
uses (scripts/groupb), seeds random graphs and random entity_ranking_scores rows (some entities get none), calls every function
for every entity, and writes the graph, the score rows and the outputs as JSON for entityColumns.golden.test.ts.
trending.today() is pinned to 2026-11-06. The container is private to the run and removed after.

    python3 api/scripts/entitycols/gen_golden.py api/src/geodb/rankedReads/__tests__/fixtures/golden-entity-columns.json
"""
import importlib.util, json, os, subprocess, sys, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
GROUPB = os.path.join(HERE, "..", "groupb")
spec = importlib.util.spec_from_file_location("groupb_gen", os.path.join(GROUPB, "gen_golden.py"))
gb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gb)

SEEDS = [11, 23, 37, 41, 59, 61, 73, 97]
COLUMNS = ["agrees", "comment_score", "commenters", "disagrees", "downvotes", "intrinsic_score", "last_listed_on",
           "listed_on_agenda_id", "participation_score", "quality_score", "ranking_score", "trending_age_credit",
           "trending_score", "trending_source", "trending_topic_id", "trending_topic_listed_on", "upvotes"]

SCORES_TABLE = """
CREATE TABLE public.entity_ranking_scores (
  entity_id uuid PRIMARY KEY, quality_score numeric NOT NULL, intrinsic_score numeric NOT NULL DEFAULT 0,
  ranking_score numeric NOT NULL, positive bigint NOT NULL DEFAULT 0, negative bigint NOT NULL DEFAULT 0,
  type_weight numeric NOT NULL DEFAULT 1.0, updated_at timestamptz NOT NULL DEFAULT now(),
  participation_score numeric NOT NULL DEFAULT 0, stance_positive bigint NOT NULL DEFAULT 0, stance_negative bigint NOT NULL DEFAULT 0,
  comment_score numeric NOT NULL DEFAULT 0, commenter_count bigint NOT NULL DEFAULT 0, watch_score numeric NOT NULL DEFAULT 0,
  age_credit numeric NOT NULL DEFAULT 0, trending_score numeric NOT NULL DEFAULT 0, trending_source text, trending_topic_id uuid);
"""

# About 70% of entities get a row. Source NULL / 'listing' / 'topic'; the topic id only with 'topic', as the refresh writes it
# (0113: CASE WHEN trending_source = 'topic' THEN topic_id END). Values include zeros, negatives and large counts.
SEED_SCORES = """
INSERT INTO public.entity_ranking_scores
  (entity_id, quality_score, intrinsic_score, ranking_score, positive, negative, participation_score, stance_positive,
   stance_negative, comment_score, commenter_count, age_credit, trending_score, trending_source, trending_topic_id)
SELECT e.id, round(random()::numeric, 9), round((random() * 0.5)::numeric, 9), round((random() * 60 - 30)::numeric, 9),
       CASE WHEN random() < 0.4 THEN 0 ELSE floor(random() * 50)::bigint END,
       CASE WHEN random() < 0.6 THEN 0 ELSE floor(random() * 9)::bigint END,
       CASE WHEN random() < 0.5 THEN 0 ELSE round((random() * 3)::numeric, 9) END,
       floor(random() * 6)::bigint, floor(random() * 6)::bigint,
       CASE WHEN random() < 0.5 THEN 0 ELSE round((random() * 2)::numeric, 9) END,
       CASE WHEN random() < 0.7 THEN 0 ELSE 3000000000 + floor(random() * 5)::bigint END,
       CASE WHEN s.src IS NULL THEN 0 ELSE round((random() * 2)::numeric, 9) END,
       CASE WHEN s.src IS NULL THEN 0 ELSE round((random() * 4)::numeric, 9) END,
       s.src, CASE WHEN s.src = 'topic' THEN pg_temp.u('topic', 1 + floor(random() * 8)::int) END
FROM public.entities e
CROSS JOIN LATERAL (SELECT CASE WHEN random() < 0.5 THEN NULL WHEN random() < 0.5 THEN 'listing' ELSE 'topic' END AS src) s
WHERE random() < 0.7;
"""


def main(out_path):
    name = "pg-entitycols-" + uuid.uuid4().hex[:8]
    gb.sh("docker", "run", "-d", "--rm", "--name", name, "-e", "POSTGRES_PASSWORD=x", "-e", "POSTGRES_INITDB_ARGS=--locale=C", "postgres:16")
    try:
        for _ in range(60):
            if subprocess.run(["docker", "exec", name, "pg_isready", "-U", "postgres"], capture_output=True).returncode == 0:
                time.sleep(1)
                break
            time.sleep(1)

        def psql(sql, **vars_):
            args = ["docker", "exec", "-i", name, "psql", "-U", "postgres", "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1"]
            for k, v in vars_.items():
                args += ["-v", f"{k}={v}"]
            return gb.sh(*args, stdin=sql)

        psql("SET TimeZone='UTC';\n" + open(os.path.join(GROUPB, "schema.sql")).read() + SCORES_TABLE)
        gb.NEEDED = gb.NEEDED + ["public.entities_" + c for c in COLUMNS]
        defs = gb.latest_definitions()
        load = "SET check_function_bodies = off;\n" + "\n".join(d for _, d in defs)
        load += "\nCREATE OR REPLACE FUNCTION trending.today() RETURNS date LANGUAGE sql STABLE AS $$ SELECT DATE '%s' $$;\n" % gb.TODAY
        psql(load)
        seed_sql = open(os.path.join(GROUPB, "seed.sql")).read()
        call = ", ".join(f"'{c}', public.entities_{c}(e)" for c in COLUMNS)
        out = {"today": gb.TODAY, "columns": COLUMNS, "seeds": []}
        for seed in SEEDS:
            tables = {
                "entities": "SELECT id, created_at FROM public.entities ORDER BY id",
                "spaces": "SELECT id, type, topic_id FROM public.spaces ORDER BY id",
                "relations": "SELECT id, type_id, from_entity_id, to_entity_id, position, space_id FROM public.relations ORDER BY id",
                "values": "SELECT id, property_id, entity_id, space_id, text, datetime_utc, datetime, date FROM public.\"values\" ORDER BY id",
                "listings": "SELECT agenda_id, space_id, entity_id, listed_on, source FROM trending.agenda_listings ORDER BY 1,2,3,5",
                "suggestions": "SELECT space_id, topic_id, name_key, links, rank FROM public.space_topic_suggestions ORDER BY 1,2",
                "scores": "SELECT entity_id, quality_score, intrinsic_score, ranking_score, positive, negative, participation_score, "
                          "stance_positive, stance_negative, comment_score, commenter_count, age_credit, trending_score, "
                          "trending_source, trending_topic_id FROM public.entity_ranking_scores ORDER BY entity_id",
                "results": f"SELECT jsonb_build_object('id', e.id, {call}) FROM public.entities e ORDER BY e.id",
            }
            script = "SET TimeZone='UTC';\n" + seed_sql + "\nTRUNCATE public.entity_ranking_scores;\n" + SEED_SCORES
            dump = "".join(f"SELECT 'T:{t}:' || coalesce(jsonb_agg(x), '[]'::jsonb)::text FROM ({q}) x;\n" for t, q in tables.items())
            rec = {"seed": seed, "tables": {}}
            for line in psql(script + dump, seed=seed).splitlines():
                if line.startswith("T:"):
                    _, t, body = line.split(":", 2)
                    rec["tables"][t] = json.loads(body)
            # jsonb_agg wraps each row as an object; 'results' rows are already objects keyed by column, unwrapped below.
            rec["tables"]["results"] = [r["jsonb_build_object"] for r in rec["tables"]["results"]]
            assert len(rec["tables"]["results"]) == len(rec["tables"]["entities"]) > 0
            out["seeds"].append(rec)
        with open(out_path, "w") as f:
            json.dump(out, f, separators=(",", ":"))
        rows = sum(len(s["tables"]["scores"]) for s in out["seeds"])
        listed = sum(1 for s in out["seeds"] for r in s["tables"]["results"] if r["last_listed_on"] is not None)
        topiclisted = sum(1 for s in out["seeds"] for r in s["tables"]["results"] if r["trending_topic_listed_on"] is not None)
        print(f"wrote {out_path}: {len(out['seeds'])} seeds, {rows} score rows, {listed} listed, {topiclisted} topic-listed")
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


if __name__ == "__main__":
    main(sys.argv[1])
