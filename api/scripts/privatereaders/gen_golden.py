#!/usr/bin/env python3
"""Golden grid for the per-user readers' twins (api/src/geodb/privateReaders.ts), from a scratch postgres:16.

Loads the LATEST definition (api/drizzle) of personalization.user_topic_contributions, user_topic_weights,
user_topic_interest_reasons and feed_user_seen over minimal copies of their tables, seeds SYNTHETIC rows from fixed seeds
(random uuids, counts, ages; nothing from a live database), calls every function for every seeded user at a pinned
as_of, and writes inputs and outputs as JSON for privateReaders.golden.test.ts. The seeds vary interest_config and
interest_signal_weights (a spread of 0, a kind that does not decay, a negative weight), and build ties on purpose: two
kinds with the same contribution (top kind by kind ASC), two sources giving a neighbour the same share (via by uuid ASC),
signals computed after as_of (age clamped to 0), a max_topics below the topic count, seen rows across UTC day edges.
The container is private to the run and removed after.

    python3 api/scripts/privatereaders/gen_golden.py api/src/geodb/__tests__/fixtures/golden-private-readers.json
"""
import json, os, random, re, subprocess, sys, time, uuid
from datetime import datetime, timedelta, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
DRIZZLE = os.path.join(HERE, "..", "..", "drizzle")
NEEDED = ["personalization.user_topic_contributions", "personalization.user_topic_weights",
          "personalization.user_topic_interest_reasons", "personalization.feed_user_seen"]
SEEDS = [3, 17, 29, 43, 71, 89]
AS_OF = "2026-11-06T12:00:00Z"
KINDS = ["vote", "comment", "debate", "follow", "interested", "not_interested"]

# The columns the functions read, as 0100 / 0109 define them (the rest of each table is irrelevant to the readers).
SCHEMA = """
CREATE SCHEMA personalization;
CREATE TABLE personalization.interest_config (
  id boolean PRIMARY KEY DEFAULT true CHECK (id), half_life_days double precision NOT NULL,
  spread_fraction double precision NOT NULL, spread_max_neighbours integer NOT NULL DEFAULT 10);
CREATE TABLE personalization.interest_signal_weights (kind text PRIMARY KEY, weight double precision NOT NULL, decays boolean NOT NULL);
CREATE TABLE personalization.user_topic_signals (
  user_id uuid NOT NULL, topic_id uuid NOT NULL, kind text NOT NULL, decayed_count double precision NOT NULL,
  event_count integer NOT NULL, last_event_at timestamptz, top_object_id uuid NOT NULL, computed_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, topic_id, kind));
CREATE TABLE personalization.topic_cooccurrence (
  topic_id uuid NOT NULL, neighbour_id uuid NOT NULL, similarity double precision NOT NULL, support integer NOT NULL,
  PRIMARY KEY (topic_id, neighbour_id));
CREATE TABLE personalization.feed_user_seen_daily (
  user_id uuid NOT NULL, day date NOT NULL, item_id uuid NOT NULL, impressions integer NOT NULL,
  last_seen_at timestamptz NOT NULL, PRIMARY KEY (user_id, day, item_id));
"""


def sh(*args, stdin=None):
    r = subprocess.run(args, input=stdin, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(r.stderr[-2000:])
    return r.stdout


def latest_definitions():
    found = {}
    for name in sorted(os.listdir(DRIZZLE)):
        if not name.endswith(".sql"):
            continue
        text = open(os.path.join(DRIZZLE, name)).read()
        for m in re.finditer(r"CREATE (?:OR REPLACE )?FUNCTION ([\w.]+)\(.*?\$\$;", text, re.S):
            found[m.group(1)] = m.group(0).replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION", 1)
    return [found[n] for n in NEEDED]


def us(dt):
    return int((dt - datetime(1970, 1, 1, tzinfo=timezone.utc)) / timedelta(microseconds=1))


def iso(dt):
    return dt.strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def seed_data(seed):
    rnd = random.Random(seed)
    uid = lambda: str(uuid.UUID(int=rnd.getrandbits(128), version=4))
    as_of = datetime.fromisoformat(AS_OF.replace("Z", "+00:00"))
    config = {"half_life_days": rnd.choice([30.0, 7.5, 90.0]), "spread_fraction": 0.0 if seed == 29 else rnd.choice([0.2, 0.35, 0.05])}
    weights = []
    for k in KINDS:
        w = {"vote": 1.0, "comment": 1.5, "debate": 2.0, "follow": 3.0, "interested": 2.0, "not_interested": -3.0}[k]
        if seed in (43, 89) and k != "interested":
            w = round(w * rnd.choice([0.5, 1.0, 1.25]), 4)
        decays = k != "follow" if seed != 71 else k not in ("follow", "comment")
        weights.append({"kind": k, "weight": w, "decays": decays})
    topics = sorted(uid() for _ in range(14))
    users = sorted(uid() for _ in range(6))
    signals = []
    for ui, u in enumerate(users):
        if ui == 0:
            continue  # a user with no signals at all
        picked = rnd.sample(topics, rnd.randint(2, 9))
        for t in picked:
            for k in rnd.sample(KINDS, rnd.randint(1, 3)):
                age = timedelta(days=rnd.uniform(-2, 200)) if rnd.random() < 0.9 else timedelta(0)
                computed = as_of - age
                signals.append({
                    "user_id": u, "topic_id": t, "kind": k,
                    "decayed_count": 0.0 if rnd.random() < 0.05 else round(rnd.uniform(0.01, 12), 6),
                    "event_count": rnd.randint(1, 40),
                    "last_event_at": None if rnd.random() < 0.15 else iso(computed - timedelta(hours=rnd.uniform(0, 500))),
                    "top_object_id": uid(),
                    "computed_at": iso(computed),
                })
        # A tie: debate and interested with the same count and age (both weight 2.0, both decay) on one fresh topic.
        if ui == 1:
            t = next(x for x in topics if x not in {s["topic_id"] for s in signals if s["user_id"] == u})
            for k in ("interested", "debate"):
                signals.append({"user_id": u, "topic_id": t, "kind": k, "decayed_count": 4.5, "event_count": 2,
                                "last_event_at": iso(as_of - timedelta(days=3)), "top_object_id": uid(),
                                "computed_at": iso(as_of - timedelta(days=3))})
        # A via tie: two topics with identical own weight, both neighbours of one target with the same similarity.
        if ui == 2:
            mine = {s["topic_id"] for s in signals if s["user_id"] == u}
            free = [x for x in topics if x not in mine]
            a, b = free[0], free[1]
            for t in (a, b):
                signals.append({"user_id": u, "topic_id": t, "kind": "follow", "decayed_count": 1.0, "event_count": 1,
                                "last_event_at": None, "top_object_id": uid(), "computed_at": iso(as_of)})
    cooc = {}
    for t in topics:
        for n in rnd.sample([x for x in topics if x != t], rnd.randint(0, 6)):
            cooc[(t, n)] = {"topic_id": t, "neighbour_id": n, "similarity": round(rnd.uniform(0.05, 1.0), 6), "support": rnd.randint(3, 90)}
    if True:
        u = users[2]
        follows = sorted(s["topic_id"] for s in signals if s["user_id"] == u and s["kind"] == "follow" and s["decayed_count"] == 1.0 and s["last_event_at"] is None)
        if len(follows) >= 2:
            a, b = follows[0], follows[1]
            target = next(x for x in topics if x not in (a, b))
            for src in (a, b):
                for key in [k for k in cooc if k[0] == src]:
                    del cooc[key]
                cooc[(src, target)] = {"topic_id": src, "neighbour_id": target, "similarity": 0.5, "support": 5}
    seen = []
    items = [uid() for _ in range(12)]
    base_day = as_of.date()
    for u in users[1:]:
        for _ in range(rnd.randint(3, 25)):
            day = base_day - timedelta(days=rnd.randint(0, 12))
            last = datetime(day.year, day.month, day.day, tzinfo=timezone.utc) + timedelta(seconds=rnd.randint(0, 86399), microseconds=rnd.randint(0, 999999))
            seen.append({"user_id": u, "day": day.isoformat(), "item_id": rnd.choice(items), "impressions": rnd.randint(1, 9),
                         "last_seen_at": iso(last)})
    dedup = {}
    for r in seen:
        dedup[(r["user_id"], r["day"], r["item_id"])] = r
    seen = sorted(dedup.values(), key=lambda r: (r["user_id"], r["day"], r["item_id"]))
    max_topics = 4 if seed in (17, 71) else 50
    return config, weights, users, signals, sorted(cooc.values(), key=lambda r: (r["topic_id"], r["neighbour_id"])), seen, max_topics


def lit(v):
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return repr(float(v)) if isinstance(v, float) else str(v)
    return "'" + str(v).replace("'", "''") + "'"


def insert(table, rows, cols):
    if not rows:
        return ""
    vals = ",\n".join("(" + ", ".join(lit(r[c]) for c in cols) + ")" for r in rows)
    return f"INSERT INTO personalization.{table} ({', '.join(cols)}) VALUES\n{vals};\n"


def main(out_path):
    name = "pg-privatereaders-" + uuid.uuid4().hex[:8]
    sh("docker", "run", "-d", "--rm", "--name", name, "-e", "POSTGRES_PASSWORD=x", "-e", "POSTGRES_INITDB_ARGS=--locale=C", "postgres:16")
    try:
        for _ in range(60):
            if subprocess.run(["docker", "exec", name, "pg_isready", "-U", "postgres"], capture_output=True).returncode == 0:
                time.sleep(1)
                break
            time.sleep(1)

        def psql(text):
            return sh("docker", "exec", "-i", name, "psql", "-U", "postgres", "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1", stdin=text)

        psql("SET TimeZone='UTC';\n" + SCHEMA + "SET check_function_bodies = off;\n" + "\n".join(latest_definitions()))
        out = {"asOf": AS_OF, "seeds": []}
        for seed in SEEDS:
            config, weights, users, signals, cooc, seen, max_topics = seed_data(seed)
            sinces = ["2026-11-06T12:00:00Z", "2026-11-03T00:00:00Z", "2026-11-01T17:30:00.25Z", "2026-10-20T00:00:00Z"]
            load = ("SET TimeZone='UTC';\nTRUNCATE personalization.interest_config, personalization.interest_signal_weights, "
                    "personalization.user_topic_signals, personalization.topic_cooccurrence, personalization.feed_user_seen_daily;\n")
            load += insert("interest_config", [config], ["half_life_days", "spread_fraction"])
            load += insert("interest_signal_weights", weights, ["kind", "weight", "decays"])
            load += insert("user_topic_signals", signals, ["user_id", "topic_id", "kind", "decayed_count", "event_count",
                                                           "last_event_at", "top_object_id", "computed_at"])
            load += insert("topic_cooccurrence", cooc, ["topic_id", "neighbour_id", "similarity", "support"])
            load += insert("feed_user_seen_daily", seen, ["user_id", "day", "item_id", "impressions", "last_seen_at"])
            psql(load)
            ts_us = lambda col: f"(extract(epoch FROM {col}) * 1000000)::bigint"
            q = {}
            for u in users:
                q[f"contributions:{u}"] = (
                    f"SELECT topic_id, kind, contribution, event_count, {ts_us('last_event_at')} AS last_event_us, top_object_id, via_topic_id "
                    f"FROM personalization.user_topic_contributions('{u}', '{AS_OF}') ORDER BY topic_id, kind, via_topic_id")
                q[f"weights:{u}"] = (
                    f"SELECT * FROM personalization.user_topic_weights('{u}', {max_topics}, '{AS_OF}')")
                for since in sinces:
                    q[f"seen:{u}:{since}"] = (
                        f"SELECT item_id, seen_count, {ts_us('last_seen_at')} AS last_seen_us FROM personalization.feed_user_seen('{u}', '{since}')")
                for t in sorted({s["topic_id"] for s in signals if s["user_id"] == u} | {c["neighbour_id"] for c in cooc}):
                    q[f"reasons:{u}:{t}"] = (
                        f"SELECT kind, contribution, event_count, {ts_us('last_event_at')} AS last_event_us, top_object_id, via_topic_id "
                        f"FROM personalization.user_topic_interest_reasons('{u}', '{t}', '{AS_OF}')")
            # row_number() over the ordered call keeps each function's own row order in the JSON.
            dump = "SET TimeZone='UTC';\n" + "".join(
                f"SELECT 'R\t{k}\t' || coalesce(jsonb_agg(to_jsonb(x) ORDER BY x.n), '[]'::jsonb)::text "
                f"FROM (SELECT row_number() OVER () AS n, y.* FROM ({sql}) y) x;\n" for k, sql in q.items())
            results = {}
            for line in psql(dump).splitlines():
                if line.startswith("R\t"):
                    _, k, body = line.split("\t", 2)
                    results[k] = [{c: v for c, v in r.items() if c != "n"} for r in json.loads(body)]
            for s in signals:
                s["last_event_us"] = None if s["last_event_at"] is None else us(datetime.fromisoformat(s["last_event_at"].replace("Z", "+00:00")))
                s["computed_us"] = us(datetime.fromisoformat(s["computed_at"].replace("Z", "+00:00")))
            for r in seen:
                r["last_seen_us"] = us(datetime.fromisoformat(r["last_seen_at"].replace("Z", "+00:00")))
            out["seeds"].append({"seed": seed, "config": config, "weights": weights, "maxTopics": max_topics, "users": users,
                                 "sinces": sinces, "signals": signals, "cooccurrence": cooc, "seen": seen, "results": results})
        with open(out_path, "w") as f:
            json.dump(out, f, separators=(",", ":"))
        n = lambda kind: sum(len(v) for s in out["seeds"] for k, v in s["results"].items() if k.startswith(kind + ":"))
        print(f"wrote {out_path}: {len(SEEDS)} seeds, {n('contributions')} contribution rows, {n('weights')} weight rows, "
              f"{n('reasons')} reason rows, {n('seen')} seen rows")
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


if __name__ == "__main__":
    main(sys.argv[1])
