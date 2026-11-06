#!/usr/bin/env python3
"""Golden grids for the group B feed and primer reads (ZP-GROUPB-READS-2), from a scratch postgres:16.

Same method as gen_golden.py: the latest definition of each function is extracted from api/drizzle, loaded next to minimal tables
(feeds_schema.sql), random graphs are seeded with setseed (feeds_seed.sql), and every read runs over a parameter grid. Output is JSON
of inputs plus outputs for the twins' tests. The anchor sets are seeded at 2026-01-01, 2026-09-01 and 2099-01-01, so primer_candidates
(which calls now()) is deterministic for any run date between September 2026 and 2099; the twins take `now`.

    python3 api/scripts/groupb/gen_golden_feeds.py api/src/geodb/rankedReads/__tests__/fixtures/golden-feeds.json
"""
import hashlib, itertools, json, os, random, re, subprocess, sys, time, uuid

HERE = os.path.dirname(os.path.abspath(__file__))
DRIZZLE = os.path.join(HERE, "..", "..", "drizzle")
SEEDS = [11, 23, 37, 41, 59, 61]
NEEDED = ["public.entities_ranked_for_feed", "public.entities_ranked_for_feed_by_type", "public.entities_ranked_for_topics",
          "public.anchor_claim_set_at", "public.primer_candidates", "public.next_primer_claim"]
CLAIM, DEBATE = "96f859ef-a1ca-4b22-9372-c86ad58b694b", "55c95b26-26f8-482c-b973-9ea99dfde438"


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


def uid(seed, kind, i):
    return str(uuid.UUID(hex=hashlib.md5(f"{seed}{kind}{i}".encode()).hexdigest()))


def main(out_path):
    name = "pg-groupb-feeds-" + uuid.uuid4().hex[:8]
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

        psql("SET TimeZone='UTC';\n" + open(os.path.join(HERE, "feeds_schema.sql")).read())
        psql("SET check_function_bodies = off;\n" + "\n".join(d for _, d in latest_definitions()))
        seed_sql = open(os.path.join(HERE, "feeds_seed.sql")).read()
        out = {"now": "2026-11-06T00:00:00Z", "seeds": []}
        for seed in SEEDS:
            rnd = random.Random(seed)
            sp = [uid(seed, "sp", i) for i in range(1, 6)]
            topics = [uid(seed, "topic", i) for i in range(1, 9)]
            types = [uid(seed, "t", i) for i in range(1, 7)]
            ents = [uid(seed, "e", i) for i in range(1, 61)]
            users = [uid(seed, "user", i) for i in range(1, 5)]
            tables = {
                "entities": "SELECT id, created_at FROM public.entities ORDER BY id",
                "spaces": "SELECT id, topic_id FROM public.spaces ORDER BY id",
                "relations": "SELECT id, type_id, from_entity_id, to_entity_id, space_id FROM public.relations ORDER BY id",
                "names": "SELECT id, entity_id, space_id, text FROM public.\"values\" ORDER BY id",
                "scores": "SELECT entity_id, ranking_score::float8 AS ranking_score FROM public.entity_ranking_scores ORDER BY 1",
                "blocklist": "SELECT entity_id FROM public.entity_feed_blocklist ORDER BY 1",
                "exclusions": "SELECT type_id FROM public.entity_type_exclusions ORDER BY 1",
                "typeRanking": "SELECT type_id, entity_id, ranking_score::float8 AS ranking_score FROM public.entity_type_ranking ORDER BY 1,2",
                "topicRanking": "SELECT topic_id, type_id, entity_id, ranking_score::float8 AS ranking_score, created_epoch FROM public.entity_topic_ranking ORDER BY 1,2,3",
                "votes": "SELECT user_id, object_id, vote_kind FROM public.user_votes ORDER BY 1,2,3",
                "stats": "SELECT claim_id, space_ids, topic_ids, weighted_voters, score, contested, engagement, name_length FROM public.primer_claim_stats ORDER BY 1",
                "anchorSets": "SELECT id, to_char(effective_from AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS effective_from FROM public.anchor_claim_sets ORDER BY 1",
                "anchorMembers": "SELECT set_id, claim_id FROM public.anchor_claim_members ORDER BY 1,2",
            }
            dump = "".join(f"SELECT 'T:{t}:' || coalesce(jsonb_agg(x), '[]'::jsonb)::text FROM ({q}) x;\n" for t, q in tables.items())
            cases = []

            def case(fn, args, call):
                aj = json.dumps(args)
                cases.append(fn)
                return f"SELECT 'C:{fn}:{aj.replace(chr(39), chr(39) * 2)}:' || coalesce(jsonb_agg(x), '[]'::jsonb)::text FROM ({call}) x;\n"

            arr = lambda xs, ty="uuid": "NULL::%s[]" % ty if xs is None else ("ARRAY[" + ",".join(f"'{x}'" for x in xs) + "]::%s[]" % ty if xs else "'{}'::%s[]" % ty)
            lit = lambda v, ty: f"NULL::{ty}" if v is None else (f"'{v}'::{ty}")
            calls = ""
            mins, afters, befores = (None, 5, 12), (None, "1760800000", "1761200000"), (None, "1761800000", "1762500000")
            spaces_opts = (None, [sp[0]], [sp[1], sp[2]], sp[:4], [])
            types_opts = (None, [types[0]], [types[0], types[1]], [CLAIM], [types[5]], [DEBATE, types[2]], [])
            feed_grid = list(itertools.product(mins, afters, befores, spaces_opts, types_opts))
            for mn, af, bf, spc, ty in rnd.sample(feed_grid, 45) + [(None, None, None, None, None)]:
                a = {"min": mn, "after": af, "before": bf, "spaces": spc, "types": ty}
                calls += case("entities_ranked_for_feed", a, f"SELECT id FROM public.entities_ranked_for_feed({lit(mn, 'numeric')}, {lit(af, 'text')}, {lit(bf, 'text')}, {arr(spc)}, {arr(ty)})")
            type_sets = (types[:2], types[:3], [CLAIM, DEBATE], [types[5]], [types[0], types[0]], [CLAIM, types[3], types[4]], [])
            for ts, mn, af, spc, cap in rnd.sample(list(itertools.product(type_sets, mins, afters, spaces_opts, (None, 1, 3, 8))), 45):
                a = {"types": ts, "min": mn, "after": af, "spaces": spc, "cap": cap}
                calls += case("entities_ranked_for_feed_by_type", a, f"SELECT id FROM public.entities_ranked_for_feed_by_type({arr(ts)}, {lit(mn, 'numeric')}, {lit(af, 'text')}, NULL::text, {arr(spc)}, {lit(cap, 'int')})")
            topic_sets = ([topics[0]], [topics[1], topics[0]], topics[:3], topics[:6], [topics[2], topics[2]], [], [topics[7]])
            type_opts = (None, [CLAIM], [DEBATE], [types[0], types[1]], [CLAIM, types[2]], [])
            topic_grid = list(itertools.product(topic_sets, (False, True), type_opts, (None, 6), (None, [sp[0]], sp[:3], []), (None, 2, 5, 20),
                                                (False, True), ("best", "new"), (False, True), (False, True)))
            for ts, wt, ty, mn, spc, cap, dtc, sb, ma, af in rnd.sample(topic_grid, 170):
                w = None if not wt else [float(rnd.randint(-3, 6)) for _ in ts]
                a = {"topics": ts, "weights": w, "types": ty, "min": mn, "after": "1760800000" if af else None, "spaces": spc, "cap": cap,
                     "debateClaims": dtc, "sort": sb, "matchAll": ma}
                calls += case("entities_ranked_for_topics", a, "SELECT id FROM public.entities_ranked_for_topics(%s, %s, %s, %s, %s, NULL::text, %s, %s, %s, %s, %s)" % (
                    arr(ts), "NULL::float8[]" if w is None else "ARRAY[%s]::float8[]" % ",".join(map(str, w)) if w else "'{}'::float8[]", arr(ty),
                    lit(mn, "numeric"), lit(a["after"], "text"), arr(spc), lit(cap, "int"), "true" if dtc else "false", lit(sb, "text"), "true" if ma else "false"))
            # the exceptions: a weight count that does not match, and an unknown sort. The golden records the SQLSTATE and message.
            for ts, w, sb in ((topics[:2], [1.0], "best"), (topics[:2], [1.0, 2.0, 3.0], "best"), (topics[:1], None, "hot"), (topics[:1], None, None)):
                a = {"topics": ts, "weights": w, "sort": sb}
                wl = "NULL::float8[]" if w is None else "ARRAY[%s]::float8[]" % ",".join(map(str, w))
                calls += case("entities_ranked_for_topics_error", a,
                              "SELECT * FROM pg_temp.try_topics(%s, %s, %s)" % (arr(ts), wl, lit(sb, "text")))
            # primer
            space_opts = (None, [], [sp[0]], [sp[1], sp[0]], sp[:3], [sp[0], sp[0], sp[1]], [sp[4]])
            primer_grid = list(itertools.product(users[:3], space_opts, (0, 1, 2, 5, 6), (0, 1, 3), (5, 3, 1), (2, 0, 4), (10, 2, 60), (3.0, 0.0, 6.0), (1.0, 0.0, 2.5), (None, 20, 50)))
            for u, spc, n_ans, n_skip, size, anch, pm, mv, nw, mx in rnd.sample(primer_grid, 80):
                a = {"user": u, "spaces": spc, "answered": n_ans, "skipped": n_skip, "size": size, "minAnchors": anch, "poolMin": pm,
                     "minVoters": mv, "noveltyWeight": nw, "maxNameChars": mx}
                # answered and skipped are the first n_ans / next n_skip claims by id in the stats table
                cond = "SELECT array_agg(claim_id ORDER BY claim_id) FROM (SELECT claim_id FROM public.primer_claim_stats ORDER BY claim_id OFFSET %d LIMIT %d) q"
                ans_sql, skip_sql = f"({cond % (0, n_ans)})", f"({cond % (n_ans, n_skip)})"
                pc = (f"public.primer_candidates('{u}', {arr(spc)}, coalesce({ans_sql}, '{{}}'), coalesce({skip_sql}, '{{}}'), {size}, {anch}, {pm}, {mv}, {nw}, {lit(mx, 'int')})")
                calls += case("primer_candidates", a, f"SELECT row_to_json(c)::jsonb AS j FROM {pc} c")
            for u, spc, n_ans, n_skip in rnd.sample(list(itertools.product(users[:3], space_opts, (0, 1, 2, 5), (0, 2))), 30):
                a = {"user": u, "spaces": spc, "answered": n_ans, "skipped": n_skip}
                cond = "SELECT array_agg(claim_id ORDER BY claim_id) FROM (SELECT claim_id FROM public.primer_claim_stats ORDER BY claim_id OFFSET %d LIMIT %d) q"
                ans_sql, skip_sql = f"({cond % (0, n_ans)})", f"({cond % (n_ans, n_skip)})"
                ncall = f"public.next_primer_claim('{u}', {arr(spc)}, coalesce({ans_sql}, '{{}}'), coalesce({skip_sql}, '{{}}'))"
                calls += case("next_primer_claim", a, f"SELECT id FROM {ncall}")
            script = "SET TimeZone='UTC';\n" + seed_sql + """
CREATE FUNCTION pg_temp.try_topics(a uuid[], w float8[], s text) RETURNS TABLE (code text, msg text) LANGUAGE plpgsql AS $f$
BEGIN
  PERFORM public.entities_ranked_for_topics(a, w, NULL, NULL, NULL, NULL, NULL, NULL, false, s, false);
  RETURN QUERY SELECT 'ok'::text, ''::text;
EXCEPTION WHEN OTHERS THEN RETURN QUERY SELECT SQLSTATE, SQLERRM;
END $f$;
"""
            lines = psql(script + dump + calls, seed=seed).splitlines()
            rec = {"seed": seed, "tables": {}, "cases": []}
            for line in lines:
                if line.startswith("T:"):
                    _, t, body = line.split(":", 2)
                    rec["tables"][t] = json.loads(body)
                elif line.startswith("C:"):
                    _, fn, rest = line.split(":", 2)
                    m = re.match(r"(\{.*?\}):(\[.*\])$", rest, re.S)
                    rows = json.loads(m.group(2))
                    if fn in ("primer_candidates",):
                        rows = [r["j"] for r in rows]
                    rec["cases"].append({"fn": fn, "args": json.loads(m.group(1)), "rows": rows})
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
