#!/usr/bin/env python3
"""Golden grids for the group B ranked reads (plan 10.22 item 8), from a scratch postgres:16.

Extracts each function's LATEST definition from api/drizzle, loads it next to minimal tables (schema.sql), seeds random graphs
with setseed (seed.sql), runs every read over a parameter grid and writes inputs plus outputs as JSON for the twins' tests.
trending.today() is pinned to 2026-11-06 (the twins take `today`). The container is private to the run and removed after.

    python3 api/scripts/groupb/gen_golden.py api/src/geodb/rankedReads/__tests__/fixtures/golden.json
"""
import json, os, re, subprocess, sys, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
DRIZZLE = os.path.join(HERE, "..", "..", "drizzle")
SEEDS = [11, 23, 37, 41, 59, 61, 73, 97]
TODAY = "2026-11-06"

NEEDED = [
    "trending.time_zone", "trending.today", "trending.value_date", "trending.position_order", "trending.claims_and_debates",
    "trending.debate_links", "trending.current_agendas", "trending.current_agenda_listings", "trending.agenda_contents",
    "trending.claim_sources", "trending.newest_listings", "trending.claim_listings", "trending.entity_listings",
    "trending.entity_topic_listings", "public.current_debate_agendas", "public.refresh_space_topic_suggestions",
    "public.topic_suggestions_for_spaces",
]


def latest_definitions():
    found = {}
    for name in sorted(os.listdir(DRIZZLE)):
        if not name.endswith(".sql"):
            continue
        text = open(os.path.join(DRIZZLE, name)).read()
        for m in re.finditer(r"CREATE (?:OR REPLACE )?FUNCTION ([\w.]+)\(.*?\$\$;", text, re.S):
            fn = m.group(1)
            fn = fn if "." in fn else "public." + fn
            found[fn] = m.group(0).replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION", 1)
    return [(n, found[n]) for n in NEEDED]


def sh(*args, stdin=None):
    r = subprocess.run(args, input=stdin, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(r.stderr[-2000:])
    return r.stdout


def main(out_path):
    name = "pg-groupb-" + uuid.uuid4().hex[:8]
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

        psql("SET TimeZone='UTC';\n" + open(os.path.join(HERE, "schema.sql")).read())
        defs = latest_definitions()
        load = "SET check_function_bodies = off;\n" + "\n".join(d for _, d in defs)
        load += "\nCREATE OR REPLACE FUNCTION trending.today() RETURNS date LANGUAGE sql STABLE AS $$ SELECT DATE '%s' $$;\n" % TODAY
        psql(load)
        seed_sql = open(os.path.join(HERE, "seed.sql")).read()
        out = {"today": TODAY, "seeds": []}
        for seed in SEEDS:
            script = "SET TimeZone='UTC';\n" + seed_sql + "\nSELECT public.refresh_space_topic_suggestions();\n"
            script = script.replace("SELECT public.refresh_space_topic_suggestions();", "SELECT public.refresh_space_topic_suggestions() \\gset\n")
            inputs = {}
            tables = {
                "entities": "SELECT id, created_at FROM public.entities ORDER BY id",
                "spaces": "SELECT id, type, topic_id FROM public.spaces ORDER BY id",
                "relations": "SELECT id, type_id, from_entity_id, to_entity_id, position, space_id FROM public.relations ORDER BY id",
                "values": "SELECT id, property_id, entity_id, space_id, text, datetime_utc, datetime, date FROM public.\"values\" ORDER BY id",
                "listings": "SELECT agenda_id, space_id, entity_id, listed_on, source FROM trending.agenda_listings ORDER BY 1,2,3,5",
                "suggestions": "SELECT space_id, topic_id, name_key, links, rank FROM public.space_topic_suggestions ORDER BY 1,2",
            }
            dump = "".join(f"SELECT 'T:{t}:' || coalesce(jsonb_agg(x), '[]'::jsonb)::text FROM ({q}) x;\n" for t, q in tables.items())
            cases = []

            def case(fn, args_json, call):
                cases.append((fn, args_json))
                return f"SELECT 'C:{fn}:{args_json.replace(chr(39), chr(39) * 2)}:' || coalesce(jsonb_agg(x), '[]'::jsonb)::text FROM ({call}) x;\n"

            ids = lambda kind, n: [str(uuid.UUID(hex=__import__('hashlib').md5(f"{seed}{kind}{i}".encode()).hexdigest())) for i in range(1, n + 1)]
            claims, debates, topics, agendas = ids("c", 14), ids("d", 10), ids("topic", 8), ids("a", 8)
            sp = ids("sp", 5)
            arr = lambda xs: "ARRAY[" + ",".join(f"'{x}'" for x in xs) + "]::uuid[]" if xs else "'{}'::uuid[]"
            calls = ""
            everything = claims + debates + topics + agendas[:2] + [str(uuid.uuid4())]
            for label, subset in (("all", everything), ("claims", claims), ("debates", debates), ("mix", claims[:5] + debates[:4] + topics[:3]), ("none", [])):
                j = json.dumps({"ids": subset})
                calls += case("claims_and_debates", j, f"SELECT * FROM trending.claims_and_debates({arr(subset)})")
                calls += case("entity_listings", j, f"SELECT * FROM trending.entity_listings({arr(subset)})")
                calls += case("entity_topic_listings", j, f"SELECT * FROM trending.entity_topic_listings({arr(subset)})")
                for srcs in (["block_item", "agenda_claims"], ["agenda_debates"], ["agenda_topics", "block_item"], []):
                    s = "ARRAY[" + ",".join(f"'{x}'" for x in srcs) + "]::text[]" if srcs else "'{}'::text[]"
                    calls += case("newest_listings", json.dumps({"ids": subset, "sources": srcs}), f"SELECT * FROM trending.newest_listings({arr(subset)}, {s})")
                calls += case("claim_listings", j, f"SELECT * FROM trending.claim_listings({arr(subset)})")
                j2 = json.dumps({"claims": subset, "debates": debates[:4] if label != "none" else []})
                calls += case("debate_links", j2, f"SELECT * FROM trending.debate_links({arr(subset)}, {arr(debates[:4] if label != 'none' else [])})")
            calls += case("current_agendas", "{}", "SELECT * FROM trending.current_agendas() ORDER BY 1,2")
            calls += case("current_agenda_listings", "{}", "SELECT * FROM trending.current_agenda_listings() ORDER BY 1,2,3,5")
            for spaces in (None, [sp[0]], [sp[1], sp[0]], sp[:3], sp, [sp[4]], []):
                for since in (None, "2026-11-01", "2026-10-20"):
                    j = json.dumps({"spaces": spaces, "since": since})
                    sa = "NULL::uuid[]" if spaces is None else arr(spaces)
                    sd = "NULL::date" if since is None else f"DATE '{since}'"
                    calls += case("agenda_contents", j, f"SELECT * FROM trending.agenda_contents({sa}, {sd}) ORDER BY 1,2,4,5,6,7,8")
                for earlier in (None, 0, 3, 10, 31, 99, -5):
                    j = json.dumps({"spaces": spaces, "earlier": earlier})
                    sa = "NULL::uuid[]" if spaces is None else arr(spaces)
                    ed = "NULL::int" if earlier is None else str(earlier)
                    calls += case("current_debate_agendas", j, f"SELECT * FROM public.current_debate_agendas({sa}, {ed})")
            for spaces in ([sp[0]], [sp[1], sp[0]], sp[:3], sp, [sp[3]], [], [str(uuid.uuid4())]):
                calls += case("topic_suggestions_for_spaces", json.dumps({"spaces": spaces}),
                              f"SELECT id, created_at FROM public.topic_suggestions_for_spaces({arr(spaces)})")
            lines = psql(script + dump + calls, seed=seed).splitlines()
            rec = {"seed": seed, "tables": {}, "cases": []}
            for line in lines:
                if line.startswith("T:"):
                    _, t, body = line.split(":", 2)
                    rec["tables"][t] = json.loads(body)
                elif line.startswith("C:"):
                    # the args JSON holds no ":" followed by a body, so split on the first two colons only after the fn name
                    _, fn, rest = line.split(":", 2)
                    m = re.match(r"(\{.*?\}):(\[.*\])$", rest, re.S)
                    rec["cases"].append({"fn": fn, "args": json.loads(m.group(1)), "rows": json.loads(m.group(2))})
            assert len(rec["cases"]) == len(cases), (len(rec["cases"]), len(cases))
            out["seeds"].append(rec)
        with open(out_path, "w") as f:
            json.dump(out, f, separators=(",", ":"))
        n = sum(len(s["cases"]) for s in out["seeds"])
        nonempty = sum(1 for s in out["seeds"] for c in s["cases"] if c["rows"])
        print(f"wrote {out_path}: {len(out['seeds'])} seeds, {n} cases, {nonempty} non-empty")
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


if __name__ == "__main__":
    main(sys.argv[1])
