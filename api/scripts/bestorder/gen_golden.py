#!/usr/bin/env python3
"""Golden grid for the ordered entity reads (plan 10.22 section 8.4, ZPL-2), from a scratch postgres:16.

Loads the LATEST definitions of public.entities_in_best_order and public.entities_ordered_by_score (api/drizzle) over minimal
tables, seeds random scores kept to few distinct values so ties are common (some entities have no score row, some ids requested
do not exist), calls both functions over a grid and writes tables, arguments and the returned id ORDER as JSON for
bestOrder.golden.test.ts. The container is private to the run and removed after.

    python3 api/scripts/bestorder/gen_golden.py api/src/geodb/rankedReads/__tests__/fixtures/golden-best-order.json
"""
import hashlib, json, os, random, re, subprocess, sys, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
DRIZZLE = os.path.join(HERE, "..", "..", "drizzle")
SEEDS = [11, 23, 37, 41, 59, 61, 73, 97]

SCHEMA = """
CREATE TYPE score_type AS ENUM ('local', 'global', 'raw');
CREATE TYPE sort_order AS ENUM ('ASC', 'DESC');
CREATE TABLE public.entities (id uuid PRIMARY KEY, created_at text NOT NULL);
CREATE TABLE public.entity_ranking_scores (entity_id uuid PRIMARY KEY, ranking_score numeric NOT NULL);
CREATE TABLE public.global_scores (entity_id uuid PRIMARY KEY, score numeric NOT NULL);
CREATE TABLE public.local_scores (entity_id uuid NOT NULL, space_id uuid NOT NULL, score numeric NOT NULL, PRIMARY KEY (entity_id, space_id));
CREATE TABLE public.votes_count (object_id uuid, object_type int, space_id uuid, vote_kind int, positive bigint, negative bigint);
"""

# 48 entities, ids from md5 so byte order is shuffled against insertion. Scores come from a handful of values so ties are common:
# ranking in 0.5-steps over [-3, 3] (about 70% scored), global about 75%, local in three spaces about 50% each.
SEED = """
SELECT setseed(:seed::int / 100.0);
TRUNCATE public.entities, public.entity_ranking_scores, public.global_scores, public.local_scores;
INSERT INTO public.entities SELECT md5('e' || :seed || '-' || n)::uuid, '1700000000' FROM generate_series(1, 48) n;
INSERT INTO public.entity_ranking_scores SELECT id, round((random() * 12)::numeric) / 2 - 3 FROM public.entities WHERE random() < 0.7;
INSERT INTO public.global_scores SELECT id, round((random() * 6)::numeric) / 2 FROM public.entities WHERE random() < 0.75;
INSERT INTO public.local_scores SELECT e.id, md5('s' || :seed || '-' || s)::uuid, round((random() * 6)::numeric) / 2
  FROM public.entities e CROSS JOIN generate_series(1, 3) s WHERE random() < 0.5;
"""


def sh(*args, stdin=None):
    r = subprocess.run(args, input=stdin, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(r.stderr[-2000:])
    return r.stdout


def latest(fn):
    found = None
    for name in sorted(os.listdir(DRIZZLE)):
        if name.endswith(".sql"):
            for m in re.finditer(r"CREATE (?:OR REPLACE )?FUNCTION " + re.escape(fn) + r"\(.*?\$\$;", open(os.path.join(DRIZZLE, name)).read(), re.S):
                found = m.group(0).replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION", 1)
    assert found, fn
    return found


def main(out_path):
    name = "pg-bestorder-" + uuid.uuid4().hex[:8]
    sh("docker", "run", "-d", "--rm", "--name", name, "-e", "POSTGRES_PASSWORD=x", "-e", "POSTGRES_INITDB_ARGS=--locale=C", "postgres:16")
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
            return sh(*args, stdin=sql)

        psql(SCHEMA + "\n" + latest("public.entities_in_best_order") + "\n" + latest("public.entities_ordered_by_score"))
        out = {"seeds": []}
        for seed in SEEDS:
            ids = [i for i in psql(SEED + "SELECT id FROM public.entities ORDER BY id;", seed=seed).split("\n")[1:] if i]
            ids = [i for i in ids if len(i) == 36]
            # Argument grid: random subsets (with duplicates and ids that do not exist), the whole table in id order, in reverse,
            # empty, and one list of exactly 1000 ids (the cap) and 1001 (the error).
            rng = random.Random(seed)
            ghost = [str(uuid.UUID(int=rng.getrandbits(128))) for _ in range(6)]
            lists = [[], ids, ids[::-1], [ghost[0]], ids[:5] + ids[:5]]
            for n in (3, 8, 15, 30, 48):
                for _ in range(3):
                    pool = ids + ghost
                    lists.append([rng.choice(pool) for _ in range(n)])
                    lists.append(rng.sample(pool, min(n, len(pool))))
            lists.append([rng.choice(ids) for _ in range(1000)])
            lists.append([rng.choice(ids) for _ in range(1001)])
            tables = {
                "entities": "SELECT id FROM public.entities ORDER BY id",
                "ranking": "SELECT entity_id, ranking_score FROM public.entity_ranking_scores ORDER BY entity_id",
                "global": "SELECT entity_id, score FROM public.global_scores ORDER BY entity_id",
                "local": "SELECT entity_id, space_id, score FROM public.local_scores ORDER BY 1,2",
            }
            spaces = sorted({str(uuid.UUID(hex=hashlib.md5(f"s{seed}-{s}".encode()).hexdigest())) for s in (1, 2, 3)})
            dump = "".join(f"SELECT 'T:{t}:' || coalesce(jsonb_agg(x), '[]'::jsonb)::text FROM ({q}) x;\n" for t, q in tables.items())
            rec = {"seed": seed, "tables": {}, "best": [], "ordered": [], "errors": []}
            for line in psql(SEED + dump, seed=seed).splitlines():
                if line.startswith("T:"):
                    _, t, body = line.split(":", 2)
                    rec["tables"][t] = json.loads(body)
            for lst in lists:
                arr = "ARRAY[" + ",".join(f"'{i}'" for i in lst) + "]::uuid[]"
                r = subprocess.run(["docker", "exec", "-i", name, "psql", "-U", "postgres", "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1"],
                                   input=f"SELECT coalesce(jsonb_agg(id::text ORDER BY n), '[]') FROM (SELECT id, row_number() OVER () n FROM public.entities_in_best_order({arr})) t;",
                                   capture_output=True, text=True)
                if r.returncode != 0:
                    assert "at most 1000 ids" in r.stderr, r.stderr
                    rec["best"].append({"ids": lst, "error": "22023"})
                else:
                    rec["best"].append({"ids": lst, "order": json.loads(r.stdout.strip())})
            # row_number() OVER () over a plpgsql SETOF numbers rows in the order the function returned them.
            for st, sp in [("global", None)] + [("local", s) for s in spaces]:
                for d in ("DESC", "ASC"):
                    spa = "NULL" if sp is None else f"'{sp}'"
                    q = f"SELECT coalesce(jsonb_agg(id::text ORDER BY n), '[]') FROM (SELECT id, row_number() OVER () n FROM public.entities_ordered_by_score('{st}', {spa}, '{d}')) t;"
                    rec["ordered"].append({"type": st, "space": sp, "direction": d, "order": json.loads(psql(q).strip())})
            for st, sp in [("local", None), (None, None)]:
                r = subprocess.run(["docker", "exec", "-i", name, "psql", "-U", "postgres", "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1"],
                                   input=f"SELECT count(*) FROM public.entities_ordered_by_score({'NULL' if st is None else repr(st)}::score_type);",
                                   capture_output=True, text=True)
                assert r.returncode != 0 and "required" in r.stderr, r.stderr
                rec["errors"].append({"type": st, "space": sp, "message": r.stderr.split("ERROR:")[1].strip().splitlines()[0]})
            out["seeds"].append(rec)
        with open(out_path, "w") as f:
            json.dump(out, f, separators=(",", ":"))
        print(f"wrote {out_path}: {len(out['seeds'])} seeds, {sum(len(s['best']) for s in out['seeds'])} best-order calls, "
              f"{sum(len(s['ordered']) for s in out['seeds'])} score-order calls")
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


if __name__ == "__main__":
    main(sys.argv[1])
