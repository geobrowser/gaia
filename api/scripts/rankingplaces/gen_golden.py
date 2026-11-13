#!/usr/bin/env python3
"""Golden output for the Best debates ranking places and push-downs (migration 0124, GEO-3681).

Runs the migrations' own SQL on a scratch Postgres that already holds the whole schema (applied by the real
migrator), over one seeded graph and a grid of scenarios, inside ONE transaction that is rolled back (so now()
is fixed for the whole run and nothing persists). Writes:

  ranking-indexer/tests/fixtures/ranking_places_golden.txt   the Rust twins' input and expected output
  api/src/geodb/rankedReads/__tests__/fixtures/ranking-places-golden.json   the TypeScript twin's

Per scenario it records what trending.current_ranking_pushdowns() and current_ranking_places() return, the ids
trending.refresh_agenda_listings(full_pass) hands to refresh_entity_ranking_scores, and, after the sweep,
trending.ranking_place_values() and trending.entity_trending_terms() over every entity. The only stubs are the
sweep's boundaries: current_agenda_listings() and current_agenda_debate_positions() return what the tables hold
(the agenda half is 0122's, pinned by its own golden), and refresh_entity_ranking_scores(ids) records the ids.

    createdb ranking_places_golden
    (cd api && DATABASE_URL_DIRECT=postgresql://.../ranking_places_golden bun drizzle-kit migrate)
    python3 api/scripts/rankingplaces/gen_golden.py postgresql://.../ranking_places_golden

Never point it at a database with content: it truncates the tables it seeds (then rolls back).
"""
import json, os, random, subprocess, sys, uuid

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
TXT = os.path.join(ROOT, "ranking-indexer", "tests", "fixtures", "ranking_places_golden.txt")
JSN = os.path.join(ROOT, "api", "src", "geodb", "rankedReads", "__tests__", "fixtures", "ranking-places-golden.json")

TYPES = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
DEBATE = "fd51f935-2063-4617-be39-7b672b23364c"
CLAIM = "96f859ef-a1ca-4b22-9372-c86ad58b694b"
DAY = 86400

rng = random.Random(3681)


def uid(group, n):
    return f"{group:08x}-0000-4000-8000-{n:012x}"


# ---- the graph --------------------------------------------------------------------------------------------------
DEB, DEB2, S1 = uid(1, 0), uid(1, 1), uid(1, 2)
BLK, BLK2, BLKX = uid(2, 0), uid(2, 1), uid(2, 2)
P = [uid(3, i) for i in range(12)]                      # personal spaces
SPACES = [(DEB, "DAO"), (DEB2, "DAO"), (S1, "DAO")] + [(p, "Personal") for p in P]
MEMBERS = [(P[i], DEB) for i in range(6)] + [(P[8], DEB2), (P[9], DEB2)]
EDITORS = [(P[6], DEB), (P[7], DEB), (P[5], DEB), (P[9], DEB2)]

# Named cases the twins' tests look up.
RANKED_NOT_LISTED, IN_BOTH, PUSHED, PUSHED_LISTED = uid(4, 0), uid(4, 1), uid(4, 2), uid(4, 3)
CASES = {"ranked_not_listed": RANKED_NOT_LISTED, "in_both": IN_BOTH, "pushed": PUSHED, "pushed_listed": PUSHED_LISTED}

entities = {}   # id -> (created offset seconds before now, or a literal text), kind
for e, age in [(RANKED_NOT_LISTED, 40 * DAY), (IN_BOTH, 2 * DAY + 3600), (PUSHED, 1 * DAY), (PUSHED_LISTED, 3 * DAY)]:
    entities[e] = (age, "debate")
debates = []
for i in range(48):
    e = uid(5, i)
    entities[e] = (rng.randrange(-DAY, 60 * DAY), "debate")
    debates.append(e)
for i in range(4):
    entities[uid(6, i)] = (rng.randrange(0, 30 * DAY), "claim")
for i in range(2):
    entities[uid(7, i)] = (rng.randrange(0, 30 * DAY), "both")
for i in range(3):
    entities[uid(8, i)] = (rng.randrange(0, 30 * DAY), "untyped")
entities[uid(9, 0)] = ("", "debate")                    # created_at not a number
entities[uid(9, 1)] = ("12x", "debate")
GONE = uid(9, 2)                                       # typed Debate, no entities row
BLOCKED = [debates[0], debates[1]]
pool = debates + [uid(6, 0), uid(6, 1), uid(7, 0), uid(8, 0), uid(9, 0), uid(9, 1), GONE]

# ranks.ranking_scores: BLK holds the cases at the top, then a shuffled pool, some entities under a second space.
scores = [(BLK, RANKED_NOT_LISTED, DEB, 1), (BLK, IN_BOTH, DEB, 2), (BLK, PUSHED, DEB, 3), (BLK, PUSHED_LISTED, DEB, 4)]
order = pool[:]
rng.shuffle(order)
for n, e in enumerate(order[:38]):
    scores.append((BLK, e, DEB, 5 + n))
for e in rng.sample(order[:38], 5):
    scores.append((BLK, e, S1, rng.randrange(1, 50)))
for n, e in enumerate(rng.sample(pool, 15)):
    scores.append((BLK2, e, DEB2, 1 + n))
for n, e in enumerate(rng.sample(pool, 10)):
    scores.append((BLKX, e, S1, 1 + n))
# Positions must be unique per (block, entity, space); the second-space rows above are distinct by space.

# Ballots: P0's one ballot holds the cases; everyone else's are random, some superseded, some tied.
ballots, items = [], []
def ballot(n, block, space, ub, ui, ents):
    rid = uid(10, n)
    ballots.append((rid, block, space, ub, ui))
    for e in ents:
        items.append((rid, e, space))
ballot(0, BLK, P[0], 100, 0, [RANKED_NOT_LISTED, IN_BOTH, PUSHED] + rng.sample(debates, 4))
n = 1
for p in P[1:11]:
    for k in range(rng.randrange(1, 4)):
        block = rng.choice([BLK, BLK, BLK2, BLKX, None])
        ub = rng.choice([10, 20, 20, 30])
        ballot(n, block, p, ub, rng.randrange(0, 3), rng.sample(debates + [uid(6, 0)], rng.randrange(0, 8)))
        n += 1
# P1 once ranked PUSHED_LISTED on a ballot it has since replaced.
ballot(n, BLK, P[1], 1, 0, [PUSHED_LISTED]); n += 1
ballot(n, BLK, P[1], 500, 0, [debates[3]]); n += 1

# Votes, as offsets around the launch time L (seconds before now). Cases first.
L = 2 * DAY
votes = [(P[1], PUSHED, 0, 0, 1, L - 3600), (P[2], PUSHED, 0, 0, 1, L - 7200),
         (P[3], PUSHED_LISTED, 0, 0, 1, L - 60), (P[6], PUSHED_LISTED, 0, 0, 1, L - 120),
         (P[10], PUSHED_LISTED, 0, 0, 1, L - 60)]
seen = {(v[0], v[1], v[2], v[3]) for v in votes}
for p in P:
    for e in rng.sample(debates + [uid(6, 1), uid(7, 1)], 14):
        ot, kind, typ = rng.choice([0, 0, 0, 0, 1]), rng.choice([0, 0, 0, 1, 3]), rng.choice([1, 1, 0])
        if (p, e, ot, kind) in seen:
            continue
        seen.add((p, e, ot, kind))
        votes.append((p, e, ot, kind, typ, L + rng.randrange(-3 * DAY, 3 * DAY)))

# Agenda listings and positions the terms read (constant across scenarios).
listings = [(uid(11, 0), S1, PUSHED_LISTED, 0, "agenda_debates"), (uid(11, 0), S1, debates[5], 0, "agenda_debates"),
            (uid(11, 1), S1, debates[6], 2, "agenda_debates"), (uid(11, 1), S1, IN_BOTH, 1, "agenda_debates")]
positions = [(uid(11, 0), S1, debates[5], 0, 1), (uid(11, 1), S1, debates[6], 2, 1), (uid(11, 1), S1, IN_BOTH, 1, 2)]
stored_terms = [(debates[7], 3.5, 0.0), (debates[8], -100.0, 0.0), (IN_BOTH, 900.0, 1.2)]

STD = [("new", DEB, BLK, 1, 7, 20, 0.65), ("all_time", DEB, BLK, 0.5, None, 30, 0.97)]
SCENARIOS = [
    # name, mode, full, since offset (None = 0), base, step, pushdown, sources, stored places, stored pushdowns
    ("off", 0, False, L, 2500, 100, 150, STD, [], []),
    ("shadow_full", 1, True, L, 2500, 100, 150, STD, [], []),
    ("shadow_hourly", 1, False, L, 2500, 100, 150, STD, "perturb", "perturb"),
    # Off with shadow's rows still stored: the full pass empties them and re-scores what they held.
    ("off_full_with_rows", 0, True, L, 2500, 100, 150, STD, "prev", "prev"),
    ("on_full", 2, True, L, 2500, 100, 150, STD, [], []),
    ("on_hourly_moves", 2, False, L, 2500, 100, 150, STD, "perturb", "perturb"),
    ("on_unchanged", 2, False, L, 2500, 100, 150, STD, "same", "same"),
    ("on_two_blocks_no_since", 2, True, None, 2500, 100, 100,
     STD + [("other", DEB2, BLK2, 1, 3, 5, 0.5)], [], []),
    ("on_retuned", 2, True, L - DAY, 1000, 37.5, 250,
     [("new", DEB, BLK, 1, 2, 3, 1), ("all_time", DEB, BLK, 0, None, 30, 0.97)], [], []),
    ("on_early_launch", 2, True, L + 4 * DAY, 2500, 100, 150, STD, [], []),
]


def q(v):
    if v is None:
        return "NULL"
    if isinstance(v, (int, float)):
        return repr(v)
    return "'" + str(v).replace("'", "''") + "'"


def main(url):
    sql = ["\\set ON_ERROR_STOP on", "BEGIN;",
           "TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores, "
           "entity_feed_blocklist, trending.agenda_listings, trending.agenda_debate_positions, "
           "trending.ranking_place_sources, trending.ranking_places, trending.ranking_pushdowns, "
           "trending.ranking_shadow_report, trending.ranking_shadow_days, ranks.rankings, ranks.ranking_items, "
           "ranks.ranking_scores, ranks.members, ranks.editors CASCADE;",
           "CREATE TEMP TABLE clock AS SELECT now() AS t, extract(epoch FROM now())::bigint AS s;",
           "SELECT 'N|' || trending.today() || '|' || (extract(epoch FROM now()))::text || '|' || (SELECT s FROM clock);"]
    for e, (c, kind) in sorted(entities.items()):
        created = f"((SELECT s FROM clock) - {c})::text" if isinstance(c, int) else q(c)
        sql.append(f"INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) "
                   f"VALUES ('{e}', {created}, '0', '0', '0');")
        for t in {"debate": [DEBATE], "claim": [CLAIM], "both": [CLAIM, DEBATE], "untyped": []}[kind]:
            sql.append(f"INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) "
                       f"VALUES (gen_random_uuid(), gen_random_uuid(), '{TYPES}', '{e}', '{t}', '{S1}', false);")
    sql.append(f"INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) "
               f"VALUES (gen_random_uuid(), gen_random_uuid(), '{TYPES}', '{GONE}', '{DEBATE}', '{S1}', false);")
    for b in BLOCKED:
        sql.append(f"INSERT INTO entity_feed_blocklist (entity_id) VALUES ('{b}');")
    for s, t in SPACES:
        sql.append(f"INSERT INTO spaces (id, type, address) VALUES ('{s}', '{t}', 'a-{s}');")
    for m, s in MEMBERS:
        sql.append(f"INSERT INTO ranks.members VALUES ('{m}', '{s}');")
    for m, s in EDITORS:
        sql.append(f"INSERT INTO ranks.editors VALUES ('{m}', '{s}');")
    for b, e, s, pos in scores:
        sql.append(f"INSERT INTO ranks.ranking_scores VALUES ('{b}', '{e}', '{s}', {1.0 / pos}, {pos});")
    for rid, b, s, ub, ui in ballots:
        sql.append(f"INSERT INTO ranks.rankings (id, block_id, space_id, updated_at_block, update_index) "
                   f"VALUES ('{rid}', {q(b)}, '{s}', {ub}, {ui});")
    for rid, e, rs in items:
        sql.append(f"INSERT INTO ranks.ranking_items (ranking_id, rank_space_id, entity_id, space_id) "
                   f"VALUES ('{rid}', '{rs}', '{e}', '{S1}');")
    for u, o, ot, kind, typ, at in votes:
        sql.append(f"INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at) "
                   f"VALUES ('{u}', '{o}', {ot}, '{rng.choice([DEB, S1, u])}', {typ}, {kind}, "
                   f"(SELECT t FROM clock) - make_interval(secs => {at}));")
    for a, s, e, d, src in listings:
        sql.append(f"INSERT INTO trending.agenda_listings VALUES ('{a}', '{s}', '{e}', trending.today() - {d}, '{src}');")
    for a, s, e, d, pos in positions:
        sql.append(f"INSERT INTO trending.agenda_debate_positions VALUES ('{a}', '{s}', '{e}', trending.today() - {d}, {pos});")
    for e, t, a in stored_terms:
        sql.append(f"INSERT INTO entity_ranking_scores (entity_id, quality_score, ranking_score, trending_score, age_credit) "
                   f"VALUES ('{e}', 0, 0, {t}, {a});")
    # The sweep's boundaries.
    sql += [
        "CREATE OR REPLACE FUNCTION trending.current_agenda_listings() RETURNS TABLE (agenda_id uuid, space_id uuid, "
        "entity_id uuid, listed_on date, source text) LANGUAGE sql STABLE AS $$ SELECT l.agenda_id, l.space_id, "
        "l.entity_id, l.listed_on, l.source FROM trending.agenda_listings l $$;",
        "CREATE OR REPLACE FUNCTION trending.current_agenda_debate_positions(since date DEFAULT NULL) RETURNS TABLE "
        "(agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, \"position\" integer) LANGUAGE sql STABLE AS "
        "$$ SELECT p.agenda_id, p.space_id, p.entity_id, p.listed_on, p.position FROM trending.agenda_debate_positions p $$;",
        "CREATE TEMP TABLE handed (scenario text, id uuid);",
        "CREATE TEMP TABLE cur (scenario text);",
        "CREATE OR REPLACE FUNCTION public.refresh_entity_ranking_scores(entity_ids uuid[]) RETURNS integer "
        "LANGUAGE sql AS $$ INSERT INTO handed SELECT (SELECT scenario FROM cur), unnest(entity_ids); "
        "SELECT cardinality(entity_ids) $$;",
        "UPDATE entity_ranking_config SET tau_seconds = 45000, trending_boost = 8, trending_topic_boost = 2.5, "
        "trending_day_factor = 0.65, trending_debate_factor = 0.75, trending_debate_window_days = 7, "
        "trending_horizon_days = 16, trending_position_base = 2500, trending_position_step = 100, "
        "trending_position_slots = 50, trending_position_day_factor = 0.3 WHERE id;",
    ]
    sql.append("SELECT 'CFG|' || tau_seconds || '|' || trending_boost || '|' || trending_topic_boost || '|' || "
               "trending_day_factor || '|' || trending_debate_factor || '|' || trending_debate_window_days || '|' || "
               "trending_horizon_days || '|' || trending_position_base || '|' || trending_position_step || '|' || "
               "trending_position_slots || '|' || trending_position_day_factor FROM entity_ranking_config WHERE id;")

    prev_places, prev_pd = "SELECT source, entity_id, place FROM trending.ranking_places", None
    for (name, mode, full, since, base, step, pushdown, sources, stored_places, stored_pd) in SCENARIOS:
        since_sql = "0" if since is None else f"(SELECT s FROM clock) - {since}"
        sql += [f"DELETE FROM cur; INSERT INTO cur VALUES ('{name}');",
                f"UPDATE entity_ranking_config SET ranking_places_mode = {mode}, ranking_place_base = {base}, "
                f"ranking_place_step = {step}, ranking_pushdown = {pushdown}, ranking_pushdown_since = {since_sql} WHERE id;",
                "UPDATE trending.refresh_state SET last_full_pass_on = trending.today() WHERE id;",
                "DELETE FROM trending.ranking_place_sources;"]
        for (sn, sp, bl, w, win, slots, f) in sources:
            sql.append(f"INSERT INTO trending.ranking_place_sources (name, space_id, block_id, weight, window_days, slots, "
                       f"day_factor) VALUES ('{sn}', '{sp}', '{bl}', {w}, {q(win)}, {slots}, {f});")
        # Stored rows: what the previous scenario left ("prev"), a perturbed copy of what the functions give now
        # ("perturb"), exactly that ("same"), or a list.
        if stored_places in ("perturb", "same"):
            sql += ["DELETE FROM trending.ranking_places; DELETE FROM trending.ranking_pushdowns;",
                    "INSERT INTO trending.ranking_pushdowns SELECT * FROM trending.current_ranking_pushdowns();",
                    "INSERT INTO trending.ranking_places SELECT * FROM trending.current_ranking_places();"]
            if stored_places == "perturb":
                sql += ["UPDATE trending.ranking_places SET place = place + 1 WHERE source = 'new' AND place IN (2, 3);",
                        "DELETE FROM trending.ranking_places WHERE source = 'all_time' AND place = 4;",
                        f"INSERT INTO trending.ranking_places VALUES ('all_time', '{debates[9]}', 31) ON CONFLICT DO NOTHING;",
                        f"DELETE FROM trending.ranking_pushdowns WHERE entity_id = '{PUSHED}';",
                        f"INSERT INTO trending.ranking_pushdowns VALUES ('{debates[10]}', 1, 0) ON CONFLICT DO NOTHING;"]
        elif stored_places != "prev":
            sql.append("DELETE FROM trending.ranking_places; DELETE FROM trending.ranking_pushdowns;")
        sql.append(f"SELECT 'SC|{name}|{mode}|{'t' if full else 'f'}|' || ranking_pushdown_since || '|' || ranking_place_base "
                   "|| '|' || ranking_place_step || '|' || ranking_pushdown FROM entity_ranking_config WHERE id;")
        sql.append(f"SELECT 'SRC|{name}|' || name || '|' || space_id || '|' || block_id || '|' || weight || '|' || "
                   "COALESCE(window_days::text, '~') || '|' || slots || '|' || day_factor FROM trending.ranking_place_sources ORDER BY name;")
        sql.append(f"SELECT 'PS|{name}|' || source || '|' || entity_id || '|' || place FROM trending.ranking_places ORDER BY 1;")
        sql.append(f"SELECT 'DS|{name}|' || entity_id FROM trending.ranking_pushdowns ORDER BY 1;")
        sql.append(f"SELECT 'XP|{name}|' || entity_id || '|' || downvoters || '|' || rankers FROM trending.current_ranking_pushdowns() ORDER BY 1;")
        sql.append(f"SELECT 'XL|{name}|' || source || '|' || entity_id || '|' || place FROM trending.current_ranking_places() ORDER BY 1;")
        sql.append(f"SELECT trending.refresh_agenda_listings({'true' if full else 'false'});")
        sql.append(f"SELECT 'IDS|{name}|' || id FROM (SELECT DISTINCT id FROM handed WHERE scenario = '{name}') x ORDER BY 1;")
        sql.append(f"SELECT 'XV|{name}|' || entity_id || '|' || place_value FROM trending.ranking_place_values("
                   "ARRAY(SELECT id FROM entities)) ORDER BY 1;")
        sql.append(f"SELECT 'TT|{name}|' || entity_id || '|' || age_credit || '|' || trending_score || '|' || "
                   "COALESCE(trending_source, '~') || '|' || listing_score || '|' || topic_score "
                   "FROM trending.entity_trending_terms(ARRAY(SELECT id FROM entities UNION SELECT entity_id FROM "
                   "trending.ranking_places)) ORDER BY 1;")
    sql.append("ROLLBACK;")
    out = subprocess.run(["psql", url, "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1"], input="\n".join(sql),
                         capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(out.stderr[-3000:])
    rows = [l for l in out.stdout.splitlines() if "|" in l and l.split("|")[0] in
            ("N", "CFG", "SC", "SRC", "PS", "DS", "XP", "XL", "IDS", "XV", "TT")]
    now_line = next(l for l in rows if l.startswith("N|"))
    now = float(now_line.split("|")[2])
    secs = int(now_line.split("|")[3])
    head = [now_line] + [l for l in rows if l.startswith("CFG|")]
    head += [f"S|{s}|{t}" for s, t in SPACES]
    for e, (c, kind) in sorted(entities.items()):
        head.append(f"E|{e}|{secs - c if isinstance(c, int) else c}")
    for e, (c, kind) in sorted(entities.items()):
        for t in {"debate": [DEBATE], "claim": [CLAIM], "both": [CLAIM, DEBATE], "untyped": []}[kind]:
            head.append(f"R|{e}|{t}|{TYPES}|{S1}")
    head.append(f"R|{GONE}|{DEBATE}|{TYPES}|{S1}")
    head += [f"BL|{b}" for b in BLOCKED]
    head += [f"M|{m}|{s}" for m, s in MEMBERS + EDITORS]
    head += [f"RS|{b}|{e}|{pos}" for b, e, s, pos in scores]
    head += [f"RB|{rid}|{b or '~'}|{s}|{ub}|{ui}" for rid, b, s, ub, ui in ballots]
    head += [f"RI|{rid}|{e}|{rs}" for rid, e, rs in items]
    # voted_at as microseconds: the clock is fixed for the transaction, so now - offset is exact.
    clock_us = None
    head += [f"C|{k}|{v}" for k, v in CASES.items()]
    head += [f"L|{a}|{s}|{e}|{d}|{src}" for a, s, e, d, src in listings]
    head += [f"P|{a}|{s}|{e}|{d}|{pos}" for a, s, e, d, pos in positions]
    head += [f"T|{e}|{t}|{a}" for e, t, a in stored_terms]
    vote_rows = [f"V|{u}|{o}|{ot}|{kind}|{typ}|{at}" for u, o, ot, kind, typ, at in votes]
    body = [l for l in rows if not l.startswith(("N|", "CFG|"))]
    with open(TXT, "w") as f:
        f.write("\n".join(head + vote_rows + body) + "\n")

    # The TypeScript twin covers the selection reads: its graph and, per scenario, XP / XL / XV.
    def scen_rows(kind, name):
        return [l.split("|")[2:] for l in body if l.startswith(kind + "|" + name + "|")]
    data = {
        "today": now_line.split("|")[1], "now": now,
        "rankable": None,
        "entities": [{"id": e, "created_at": str(secs - c) if isinstance(c, int) else c,
                      "kind": kind} for e, (c, kind) in sorted(entities.items())],
        "gone": GONE, "blocked": BLOCKED,
        "members": [{"member_space_id": m, "space_id": s} for m, s in MEMBERS + EDITORS],
        "scores": [{"block_id": b, "entity_id": e, "position": pos} for b, e, s, pos in scores],
        "ballots": [{"id": rid, "block_id": b, "space_id": s, "updated_at_block": ub, "update_index": ui}
                    for rid, b, s, ub, ui in ballots],
        "items": [{"ranking_id": rid, "entity_id": e, "rank_space_id": rs} for rid, e, rs in items],
        "votes": [{"user_id": u, "object_id": o, "object_type": ot, "vote_kind": kind, "vote_type": typ,
                   "voted_at_offset": at} for u, o, ot, kind, typ, at in votes],
        "cases": CASES,
        "scenarios": [],
    }
    for sc in [l.split("|") for l in body if l.startswith("SC|")]:
        name = sc[1]
        data["scenarios"].append({
            "name": name, "since": int(sc[4]), "base": float(sc[5]), "step": float(sc[6]),
            "sources": [{"name": r[0], "space_id": r[1], "block_id": r[2], "weight": float(r[3]),
                         "window_days": None if r[4] == "~" else int(r[4]), "slots": int(r[5]), "day_factor": float(r[6])}
                        for r in scen_rows("SRC", name)],
            "places_after": [{"source": r[0], "entity_id": r[1], "place": int(r[2])} for r in scen_rows("XL", name)]
            if sc[2] != "0" else [],
            "pushdowns": [{"entity_id": r[0], "downvoters": int(r[1]), "rankers": int(r[2])} for r in scen_rows("XP", name)],
            "places": [{"source": r[0], "entity_id": r[1], "place": int(r[2])} for r in scen_rows("XL", name)],
            "values": [{"entity_id": r[0], "place_value": r[1]} for r in scen_rows("XV", name)],
        })
    with open(JSN, "w") as f:
        json.dump(data, f, indent="\t")
        f.write("\n")
    # CI's format check covers fixtures too.
    biome = os.path.join(ROOT, "api", "node_modules", ".bin", "biome")
    if os.path.exists(biome):
        subprocess.run([biome, "format", "--write", JSN], capture_output=True)
    print(f"{len(head) + len(vote_rows) + len(body)} rows, {len(SCENARIOS)} scenarios")


if __name__ == "__main__":
    main(sys.argv[1])
