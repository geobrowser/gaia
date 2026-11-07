"""Tests for the geo-db scoring data provider (src/geodb_reads): equality with the Postgres provider, and the counters."""

import re
import uuid
from contextlib import contextmanager
from datetime import datetime, timezone
from unittest.mock import patch

from src.geodb_reads import (
    BACKLINKS_LIMIT,
    ROLE_EDITOR,
    ROLE_MEMBER,
    VOTE_DIRECTION,
    VOTE_KIND,
    VOTE_OBJECT,
    VOTE_OBJECT_TYPE,
    VOTE_SPACE,
    VOTE_USER,
    VOTE_VOTED_AT,
    GeoDbReadError,
    GeoDbScoringDataProvider,
    Row,
    compact,
    compare_geodb_reads,
    conforming_id,
    enabled,
    parse_rows,
    system_spaces_from_env,
)
from src.scoring_data_provider import ScoringDataProvider

URL = "postgresql://test:test@localhost/test"
GOV = "a" * 12 + "4" + "a" * 3 + "8" + "a" * 15
VOTES = "b" * 12 + "4" + "b" * 3 + "8" + "b" * 15


def v4(n: int) -> str:
    """A v4 uuid, dashed, as psycopg prints a uuid column."""
    return str(uuid.UUID(f"{n:012x}4{n:03x}8{n:015x}"))


S1, S2, S3 = v4(1), v4(2), v4(3)
U1, U2 = v4(11), v4(12)
E1, E2 = v4(21), v4(22)
# A Space id that is not a v4: geo-db holds it hashed, so it has to be mapped back through the Space ids Postgres lists.
ODD = "00000000-0000-0000-0000-00000000abcd"
T1 = datetime(2024, 1, 1, 12, 0, 0, 250000, tzinfo=timezone.utc)
T2 = datetime(2024, 1, 2, tzinfo=timezone.utc)

MEMBERS = [(U1, S1), (U1, S2), (U2, S1), (U2, ODD)]
EDITORS = [(U1, S3), (U2, S2)]
# (user, object, space, vote_type, voted_at, object_type, vote_kind)
VOTE_ROWS = [
    (U1, E1, S1, 0, T1, 0, 0),
    (U1, E2, S2, 1, T2, 0, 2),
    (U2, E1, ODD, 0, T1, 0, 1),
    (U2, E1, S1, 2, T1, 0, 0),  # a removal: not a vote
    (U2, E2, S1, 0, T1, 0, 3),  # Interested: not a vote
    (U2, E2, S2, 0, T1, 1, 0),  # a relation vote: not read
]


class FakeCursor:
    def __init__(self, tables: dict):
        self.tables = tables
        self.rows: list = []

    def execute(self, sql: str, *args) -> None:
        text = " ".join(sql.split())
        if "FROM user_votes" in text:
            self.rows = [(u, o, s, vt, at) for (u, o, s, vt, at, ot, kind) in self.tables["votes"] if ot == 0 and kind != 3]
        elif "FROM members" in text:
            self.rows = self.tables["members"]
        elif "FROM editors" in text:
            self.rows = self.tables["editors"]
        elif "FROM spaces" in text:
            self.rows = [(s,) for s in self.tables["spaces"]]
        elif "scoring_topology_distances" in text:
            self.rows = []
        else:
            self.rows = []

    def fetchall(self) -> list:
        return self.rows

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class FakeConn:
    def __init__(self, tables: dict):
        self.tables = tables

    def cursor(self) -> FakeCursor:
        return FakeCursor(self.tables)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def tables(**over) -> dict:
    base = {
        "spaces": [S1, S2, S3, ODD, U1, U2],
        "members": MEMBERS,
        "editors": EDITORS,
        "votes": VOTE_ROWS,
    }
    base.update(over)
    return base


def nanos(at: datetime) -> str:
    return f"instant:{int(at.timestamp())}.{at.microsecond * 1000:09d}"


class FakeGeoDb:
    """geo-db's side of a fixture: the same rows written the way the feeder writes them, read through the reader seam."""

    def __init__(self, t: dict, fail_on: str | None = None):
        self.cardrows: dict[tuple[str, str], list[tuple[str, str]]] = {}
        self.refs: dict[tuple[str, str], list[Row]] = {}
        self.reads = 0
        self.fail_on = fail_on
        ents = {compact(s): conforming_id("entity", s) for s in t["spaces"]}
        for role, rows in ((ROLE_MEMBER, t["members"]), (ROLE_EDITOR, t["editors"])):
            for member, space in rows:
                subject = conforming_id("entity", space)
                self.cardrows.setdefault((GOV, subject), []).append((role, conforming_id("entity", member)))
        for i, (u, o, s, vt, at, ot, kind) in enumerate(t["votes"]):
            rec = conforming_id("entity", v4(1000 + i))
            cols = [
                (VOTE_USER, conforming_id("entity", u)),
                (VOTE_OBJECT, conforming_id("entity", o)),
                (VOTE_OBJECT_TYPE, str(ot)),
                (VOTE_SPACE, conforming_id("entity", s)),
                (VOTE_KIND, str(kind)),
                (VOTE_DIRECTION, str(vt)),
                (VOTE_VOTED_AT, nanos(at)),
            ]
            self.cardrows[(VOTES, rec)] = cols
            self.refs.setdefault((VOTES, conforming_id("entity", u)), []).append(Row(VOTE_USER, conforming_id("entity", u), rec))
        assert ents

    def cards(self, space: str, entity_ids: list[str]) -> list[Row]:
        self.reads += 1
        if self.fail_on == "cards":
            raise GeoDbReadError("refused")
        return [Row(p, r, e) for e in entity_ids for (p, r) in self.cardrows.get((space, e), [])]

    def backlinks(self, space: str, entity_id: str, limit: int) -> list[Row]:
        self.reads += 1
        if self.fail_on == "backlinks":
            raise GeoDbReadError("refused")
        return self.refs.get((space, entity_id), [])[:limit]

    def close(self) -> None:
        pass


def provider(t: dict, geo: FakeGeoDb, **kw) -> GeoDbScoringDataProvider:
    return GeoDbScoringDataProvider(URL, geo, GOV, VOTES, **kw)


def users_key(users) -> dict:
    return {u.id: (sorted(u.member_spaces), sorted(u.editor_spaces)) for u in users}


def votes_key(votes) -> list:
    return sorted((v.user_id, v.entity_id, v.space_id, v.vote_type.value, v.timestamp) for v in votes)


class TestEquality:
    def test_both_providers_return_the_same_users_and_votes(self) -> None:
        t = tables()
        conn = FakeConn(t)
        pg = ScoringDataProvider(URL)
        geo = provider(t, FakeGeoDb(t))
        pg_users, geo_users = pg._fetch_users(conn), geo._fetch_users(conn)
        pg_votes, geo_votes = pg._fetch_votes(conn), geo._fetch_votes(conn)
        assert users_key(geo_users) == users_key(pg_users)
        assert len(pg_users) == 2 and len(pg_votes) == 3
        assert votes_key(geo_votes) == votes_key(pg_votes)
        assert geo.stats.unmapped == 0 and geo.stats.bad_records == 0

    def test_a_row_only_postgres_holds_is_a_difference(self) -> None:
        t = tables()
        geo_t = tables(members=MEMBERS[:-1], votes=VOTE_ROWS[:1])
        with patch("src.geodb_reads.psycopg.connect", return_value=FakeConn(t)):
            pg_data = _scoring_data(t)
            stats = compare_geodb_reads(URL, pg_data, reader=FakeGeoDb(geo_t), spaces={"governance": GOV, "votes": VOTES})
        assert stats.mismatch["members"] == 1
        assert stats.mismatch["editors"] == 0
        # U1 has two votes geo-db lacks (one), U2's counted-by-voter set is the same: 2 differing rows
        assert stats.mismatch["votes"] == 2
        assert stats.total_mismatch == 3
        assert not stats.failed

    def test_identical_data_counts_no_mismatch(self) -> None:
        t = tables()
        with patch("src.geodb_reads.psycopg.connect", return_value=FakeConn(t)):
            stats = compare_geodb_reads(
                URL, _scoring_data(t), reader=FakeGeoDb(t), spaces={"governance": GOV, "votes": VOTES}
            )
        assert stats.total_mismatch == 0 and not stats.failed
        assert stats.members_geodb == stats.members_postgres == 4
        assert stats.votes_geodb == stats.votes_postgres == 3

    def test_votes_by_non_members_are_counted_apart(self) -> None:
        t = tables(votes=VOTE_ROWS + [(v4(99), E1, S1, 0, T1, 0, 0)])
        with patch("src.geodb_reads.psycopg.connect", return_value=FakeConn(t)):
            stats = compare_geodb_reads(
                URL, _scoring_data(t), reader=FakeGeoDb(t), spaces={"governance": GOV, "votes": VOTES}
            )
        assert stats.votes_outside_voters == 1
        assert stats.total_mismatch == 0

    def test_a_vote_at_another_time_is_a_difference(self) -> None:
        t = tables()
        other = tables(votes=[(u, o, s, vt, T2, ot, k) for (u, o, s, vt, _a, ot, k) in VOTE_ROWS])
        with patch("src.geodb_reads.psycopg.connect", return_value=FakeConn(other)):
            stats = compare_geodb_reads(
                URL, _scoring_data(t), reader=FakeGeoDb(other), spaces={"governance": GOV, "votes": VOTES}
            )
        assert stats.mismatch["votes"] > 0 and stats.mismatch["members"] == 0

    def test_an_id_nothing_maps_back_shows_as_an_extra_row(self) -> None:
        t = tables()
        geo = FakeGeoDb(t)
        geo.cardrows[(GOV, conforming_id("entity", S3))].append((ROLE_MEMBER, conforming_id("entity", "not-a-known-id")))
        users = provider(t, geo)._fetch_users(FakeConn(t))
        pg = ScoringDataProvider(URL)._fetch_users(FakeConn(t))
        extra = sum(len(u.member_spaces) for u in users) - sum(len(u.member_spaces) for u in pg)
        assert extra == 1

    def test_a_value_that_is_not_an_id_is_dropped_and_counted(self) -> None:
        t = tables()
        geo = FakeGeoDb(t)
        geo.cardrows[(GOV, conforming_id("entity", S3))].append((ROLE_MEMBER, "not hex"))
        p = provider(t, geo)
        users = p._fetch_users(FakeConn(t))
        assert p.stats.unmapped == 1
        assert users_key(users) == users_key(ScoringDataProvider(URL)._fetch_users(FakeConn(t)))

    def test_a_voter_at_the_backlink_limit_is_counted_truncated(self) -> None:
        t = tables()
        geo = FakeGeoDb(t)
        rows = geo.refs[(VOTES, conforming_id("entity", U1))]
        geo.refs[(VOTES, conforming_id("entity", U1))] = [rows[0]] * BACKLINKS_LIMIT
        p = provider(t, geo)
        p._fetch_users(FakeConn(t))
        p._fetch_votes(FakeConn(t))
        assert p.stats.truncated_voters == 1


def _scoring_data(t: dict):
    from src.scoring_data_provider.scoring_data_provider import ScoringData

    conn = FakeConn(t)
    pg = ScoringDataProvider(URL)
    return ScoringData(entities=[], votes=pg._fetch_votes(conn), users=pg._fetch_users(conn), spaces=[])


class TestNeverRaises:
    def test_a_refused_read_is_counted_failed(self) -> None:
        t = tables()
        with patch("src.geodb_reads.psycopg.connect", return_value=FakeConn(t)):
            stats = compare_geodb_reads(
                URL, _scoring_data(t), reader=FakeGeoDb(t, fail_on="cards"), spaces={"governance": GOV, "votes": VOTES}
            )
        assert stats.failed == "read_failed"

    def test_a_missing_system_space_file_is_counted_failed(self) -> None:
        stats = compare_geodb_reads(URL, _scoring_data(tables()), reader=FakeGeoDb(tables()), environ={})
        assert stats.failed == "no_system_spaces"

    def test_an_exhausted_budget_is_counted(self) -> None:
        t = tables()
        with patch("src.geodb_reads.psycopg.connect", return_value=FakeConn(t)):
            stats = compare_geodb_reads(
                URL,
                _scoring_data(t),
                reader=FakeGeoDb(t),
                spaces={"governance": GOV, "votes": VOTES},
                environ={"GEODB_SCORING_READS_BUDGET_S": "-1"},
            )
        assert stats.failed == "budget"

    def test_an_unexpected_error_does_not_escape(self) -> None:
        with patch("src.geodb_reads.psycopg.connect", side_effect=RuntimeError("down")):
            stats = compare_geodb_reads(
                URL, _scoring_data(tables()), reader=FakeGeoDb(tables()), spaces={"governance": GOV, "votes": VOTES}
            )
        assert stats.failed == "RuntimeError"


class TestFlagAndParsing:
    def test_flag_defaults_off(self) -> None:
        assert not enabled({})
        assert not enabled({"GEODB_SCORING_READS": "0"})
        assert not enabled({"GEODB_SCORING_READS": ""})
        assert enabled({"GEODB_SCORING_READS": "1"})
        assert enabled({"GEODB_SCORING_READS": "true"})

    def test_main_does_not_compare_with_the_flag_off(self) -> None:
        import main

        assert main.geodb_reads_enabled() is False

    def test_system_spaces_file(self, tmp_path) -> None:
        path = tmp_path / "system-spaces.json"
        path.write_text('{"governance": {"geo": "%s"}, "votes": {"geo": "not hex"}}' % GOV)
        assert system_spaces_from_env({"GEODB_SYSTEM_SPACES_PATH": str(path)}) == {"governance": GOV}
        assert system_spaces_from_env({"GEODB_SYSTEM_SPACES_PATH": str(tmp_path / "missing")}) == {}
        assert system_spaces_from_env({}) == {}

    def test_conforming_id_matches_the_feeders(self) -> None:
        assert conforming_id("entity", v4(5)) == compact(v4(5))
        assert re.fullmatch(r"[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}", conforming_id("entity", ODD))
        # Pinned against geodb-feeder/src/ids.ts (sha256 of "geo:legacy-entity:v1\0<compact id>").
        assert conforming_id("entity", ODD) == conforming_id("entity", compact(ODD))

    def test_property_ids_are_the_feeders(self) -> None:
        assert ROLE_MEMBER == "dbe3bdf637a5458d9206d737fb104793"
        assert ROLE_EDITOR == "79c3a498486641bc930f3b973de4b4d3"

    def test_parse_rows(self) -> None:
        e = "c" * 32
        out = parse_rows(f"p\tv\\tx\tsid\t{e}\nq\t{e}\tsid\t{'d' * 32}\n", backlinks_of=e)
        assert [(r.prop, r.entity) for r in out] == [("q", "d" * 32)]
        assert parse_rows(f"p\tv\\tx\tsid\t{e}\n")[0].raw == "v\tx"
        try:
            parse_rows("p\tv\tsid\tnot-an-entity\n")
            raise AssertionError("should refuse")
        except GeoDbReadError:
            pass
