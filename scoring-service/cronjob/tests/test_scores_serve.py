"""Tests for the scores serve path (GEODB_SCORES_SERVE): the writer's changed-rows diff read from geo-db, and its fallback."""

import sys
import uuid
from datetime import datetime
from unittest.mock import MagicMock, patch

import pytest

from src.algorithm.models import Entity, Perspective
from src.geodb_reads import (
    COLLECTION_GLOBAL,
    COLLECTION_LOCAL,
    DerivedGetReader,
    GeoDbReadError,
    GeoDbTimeout,
    ScanPage,
    ScoresServe,
    StoredScore,
    serve_enabled,
)
from src.scoring_data_writer import ScoringDataWriter

E1 = "11111111-1111-4111-8111-111111111111"
E2 = "22222222-2222-4222-8222-222222222222"
S1 = "33333333-3333-4333-8333-333333333333"
ZERO = "0" * 32


def entity(eid: str, score: float, perspectives=()) -> Entity:
    e = Entity(id=eid, created_at=datetime.now())
    e.normalized_score = score
    e.perspectives = list(perspectives)
    return e


def persp(eid: str, sid: str, score: float) -> Perspective:
    p = Perspective(id=f"{eid}_{sid}", entity_id=eid, space_id=sid, created_at=datetime.now())
    p.normalized_score = score
    return p


class FakeReader:
    """Holds rows by (collection, key hex); can fail or tick a fake clock on each read."""

    def __init__(self, rows=None, error: Exception | None = None, tick=None, page: int = 2):
        self.rows = rows or {}
        self.error = error
        self.tick = tick
        self.page = page
        self.reads = 0
        self.scans = 0
        self.closed = 0

    def scan(self, collection: int, group_hex: str, after: str | None) -> ScanPage:
        """geo-db's order is score descending then key ascending; the cursor here is the index of the next row."""
        self.scans += 1
        if self.tick:
            self.tick()
        if self.error:
            raise self.error
        rows = sorted(
            ((v.score, k[1], v.value) for k, v in self.rows.items() if k[0] == collection and v.group == group_hex),
            key=lambda r: (-r[0], r[1]),
        )
        start = int(after) if after else 0
        chunk = rows[start:start + self.page]
        more = start + self.page < len(rows)
        return ScanPage(chunk, str(start + self.page) if more else None)

    def get(self, collection: int, key_hex: str):
        self.reads += 1
        if self.tick:
            self.tick()
        if self.error:
            raise self.error
        return self.rows.get((collection, key_hex))

    def close(self) -> None:
        self.closed += 1


def stored_for(entities) -> dict:
    rows = {}
    for e in entities:
        eb = uuid.UUID(e.id).bytes
        rows[(COLLECTION_GLOBAL, eb.hex())] = StoredScore(ZERO, e.normalized_score, "")
        for p in e.perspectives:
            sb = uuid.UUID(p.space_id).bytes
            rows[(COLLECTION_LOCAL, (eb + sb).hex())] = StoredScore(sb.hex(), p.normalized_score, "")
    return rows


def data():
    return [entity(E1, 0.5, [persp(E1, S1, 0.7)]), entity(E2, 0.6)]


class TestFlag:
    def test_off_by_default(self) -> None:
        assert not serve_enabled({})
        assert ScoresServe.from_env({}) is None

    def test_scoring_reads_flag_does_not_turn_it_on(self) -> None:
        assert not serve_enabled({"GEODB_SCORING_READS": "1"})

    @pytest.mark.parametrize("v", ["1", "true", "TRUE"])
    def test_on(self, v: str) -> None:
        assert serve_enabled({"GEODB_SCORES_SERVE": v})

    def test_on_without_key_counts_misconfigured_and_falls_back(self) -> None:
        serve = ScoresServe.from_env({"GEODB_SCORES_SERVE": "1"})
        assert serve is not None
        assert serve.changed_keys("global_scores", data()) is None
        assert serve.stats.fallback["misconfigured"] == 1


class TestServed:
    def test_matching_rows_are_served_as_unchanged(self) -> None:
        ents = data()
        serve = ScoresServe(FakeReader(stored_for(ents)))
        assert serve.changed_keys("global_scores", ents) == set()
        assert serve.changed_keys("local_scores", ents) == set()
        assert serve.stats.served == {"global_scores": 1, "local_scores": 1}
        assert sum(serve.stats.fallback.values()) == 0

    def test_changed_missing_and_wrong_group_rows_are_listed(self) -> None:
        ents = data()
        rows = stored_for(ents)
        rows[(COLLECTION_GLOBAL, uuid.UUID(E1).bytes.hex())] = StoredScore(ZERO, 0.4, "")  # score differs
        del rows[(COLLECTION_GLOBAL, uuid.UUID(E2).bytes.hex())]  # geo-db holds none
        key = (COLLECTION_LOCAL, (uuid.UUID(E1).bytes + uuid.UUID(S1).bytes).hex())
        rows[key] = StoredScore(ZERO, 0.7, "")  # wrong group
        serve = ScoresServe(FakeReader(rows))
        assert serve.changed_keys("global_scores", ents) == {E1, E2}
        assert serve.changed_keys("local_scores", ents) == {(E1, S1)}

    def test_the_diff_pages_a_group_instead_of_reading_a_row_at_a_time(self) -> None:
        ents = [entity(f"{i:08x}-1111-4111-8111-111111111111", 0.01 * i) for i in range(1, 8)]
        reader = FakeReader(stored_for(ents), page=3)
        serve = ScoresServe(reader)
        assert serve.changed_keys("global_scores", ents) == set()
        assert reader.reads == 0
        assert reader.scans == 3  # 7 rows, 3 a page
        assert serve.stats.rows_read == 7

    def test_a_row_moved_between_pages_is_still_found_changed(self) -> None:
        ents = [entity(f"{i:08x}-1111-4111-8111-111111111111", 0.01 * i) for i in range(1, 8)]
        rows = stored_for(ents)
        moved = ents[3]
        rows[(COLLECTION_GLOBAL, uuid.UUID(moved.id).bytes.hex())] = StoredScore(ZERO, 0.99, "")
        assert ScoresServe(FakeReader(rows, page=2)).changed_keys("global_scores", ents) == {moved.id}

    def test_local_scores_scan_each_space_once(self) -> None:
        s2 = "44444444-4444-4444-8444-444444444444"
        ents = [entity(E1, 0.5, [persp(E1, S1, 0.7), persp(E1, s2, 0.2)]), entity(E2, 0.6, [persp(E2, S1, 0.1)])]
        reader = FakeReader(stored_for(ents), page=1)
        assert ScoresServe(reader).changed_keys("local_scores", ents) == set()
        assert reader.scans == 3  # S1 holds two rows (a page each), s2 one: a Space is scanned once

    def test_a_scan_cursor_that_does_not_advance_falls_back(self) -> None:
        reader = FakeReader(stored_for(data()))
        reader.scan = lambda c, g, after: ScanPage([], "aa")  # type: ignore[method-assign]
        serve = ScoresServe(reader)
        assert serve.changed_keys("global_scores", data()) is None
        assert serve.stats.fallback["error"] == 1

    def test_row_with_a_value_counts_as_changed(self) -> None:
        ents = data()
        rows = stored_for(ents)
        rows[(COLLECTION_GLOBAL, uuid.UUID(E1).bytes.hex())] = StoredScore(ZERO, 0.5, "ab")
        assert ScoresServe(FakeReader(rows)).changed_keys("global_scores", ents) == {E1}

    def test_writer_uses_the_served_diff_and_skips_the_postgres_one(self) -> None:
        ents = data()
        serve = ScoresServe(FakeReader(stored_for(ents)))
        changed = self._run_writer(serve, ents)
        assert changed == {"global_scores": set(), "local_scores": set()}

    @staticmethod
    def _run_writer(serve, ents, pg_changed=None):
        w = ScoringDataWriter("postgresql://x", derived=None, serve=serve)
        # No mirror is configured, so the writer would not read the state; give it the state of a synced table.
        pg = MagicMock(side_effect=lambda conn, t, e: (pg_changed or {})[t]) if pg_changed else MagicMock(
            side_effect=AssertionError("Postgres diff must not run")
        )
        seen: dict = {}
        with (
            patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as c,
            patch.object(ScoringDataWriter, "_mirror_state",
                         return_value={t: (True, None) for t in ("global_scores", "local_scores")}),
            patch.object(ScoringDataWriter, "_changed_keys", pg),
            patch.object(ScoringDataWriter, "_write_global_scores"),
            patch.object(ScoringDataWriter, "_write_local_scores"),
            patch.object(ScoringDataWriter, "_write_space_scores"),
            patch.object(ScoringDataWriter, "_set_state"),
            patch.object(ScoringDataWriter, "_mirror_to_geodb",
                         side_effect=lambda ents_, sp, changed, tables, resume: seen.update(changed)),
        ):
            c.return_value.__enter__.return_value = MagicMock()
            # _mirror_state only runs for the tables of an enabled mirror; stand one in.
            w._derived = MagicMock()
            w._derived.enabled.return_value = True
            w.write_all(ents, [])
        return seen


class TestFallback:
    def test_error_falls_back_and_counts_error(self) -> None:
        reader = FakeReader(error=GeoDbReadError("geodb-client closed"))
        serve = ScoresServe(reader)
        assert serve.changed_keys("global_scores", data()) is None
        assert serve.stats.fallback == {"error": 1}
        assert reader.closed == 1

    def test_unexpected_exception_is_an_error_too(self) -> None:
        serve = ScoresServe(FakeReader(error=RuntimeError("boom")))
        assert serve.changed_keys("local_scores", data()) is None
        assert serve.stats.fallback["error"] == 1

    def test_client_timeout_falls_back_and_counts_timeout(self) -> None:
        serve = ScoresServe(FakeReader(error=GeoDbTimeout("geodb-client timed out")))
        assert serve.changed_keys("global_scores", data()) is None
        assert serve.stats.fallback == {"timeout": 1}

    def test_budget_overrun_falls_back_and_counts_timeout(self) -> None:
        now = [0.0]
        reader = FakeReader(stored_for(data()), tick=lambda: now.__setitem__(0, now[0] + 700.0), page=1)
        serve = ScoresServe(reader, budget_s=600.0, clock=lambda: now[0])
        assert serve.changed_keys("global_scores", data()) is None
        assert serve.stats.fallback == {"timeout": 1}

    def test_counter_renders_every_reason_at_zero(self) -> None:
        serve = ScoresServe(FakeReader(error=GeoDbReadError("x")))
        serve.changed_keys("global_scores", data())
        assert serve.stats.render().splitlines() == [
            "# TYPE gaia_scoring_scores_serve_fallback_total counter",
            'gaia_scoring_scores_serve_fallback_total{reason="error"} 1',
            'gaia_scoring_scores_serve_fallback_total{reason="timeout"} 0',
            'gaia_scoring_scores_serve_fallback_total{reason="misconfigured"} 0',
        ]

    def test_writer_falls_back_to_the_postgres_diff(self) -> None:
        serve = ScoresServe(FakeReader(error=GeoDbReadError("x")))
        pg = {"global_scores": {E1}, "local_scores": {(E1, S1)}}
        changed = TestServed._run_writer(serve, data(), pg_changed=pg)
        assert changed == pg
        assert serve.stats.fallback["error"] == 2  # one per diffed table


class TestDerivedGetReader:
    def _reader(self, tmp_path, answer: str) -> DerivedGetReader:
        p = tmp_path / "fake.py"
        p.write_text(
            'import sys\nprint("ready", flush=True)\nfor l in sys.stdin:\n'
            f"    sys.stdout.write({answer!r})\n    sys.stdout.flush()\n"
        )
        return DerivedGetReader([sys.executable, str(p)], timeout=5.0)

    def test_none_and_row(self, tmp_path) -> None:
        r = self._reader(tmp_path, "+4\nnone")
        assert r.get(2, "aa") is None
        r.close()
        row = f"{ZERO}\t0.5\t"
        r = self._reader(tmp_path, f"+{len(row)}\n{row}")
        assert r.get(2, "aa") == StoredScore(ZERO, 0.5, "")
        r.close()

    def test_malformed_answer_is_a_read_error(self, tmp_path) -> None:
        r = self._reader(tmp_path, "+3\nabc")
        with pytest.raises(GeoDbReadError):
            r.get(2, "aa")
        r.close()

    def test_silent_child_times_out(self, tmp_path) -> None:
        p = tmp_path / "silent.py"
        p.write_text('import sys, time\nprint("ready", flush=True)\ntime.sleep(30)\n')
        r = DerivedGetReader([sys.executable, str(p)], timeout=0.5)
        with pytest.raises(GeoDbTimeout):
            r.get(2, "aa")
        r.close()

    def test_from_env_builds_the_derived_pipe_argv(self) -> None:
        r = DerivedGetReader.from_env({"GEODB_KEY_PATH": "/k", "GEODB_BINDING_PATH": "/b"})
        assert r is not None and r._argv[-2:] == ["derived", "pipe"]
        assert DerivedGetReader.from_env({}) is None


class TestDerivedScanPages:
    def _reader(self, tmp_path, answers: list[str]) -> DerivedGetReader:
        p = tmp_path / "scan.py"
        p.write_text(
            "import sys\nanswers = %r\nlines = []\nprint('ready', flush=True)\nfor l in sys.stdin:\n"
            "    lines.append(l)\n    a = answers.pop(0)\n    sys.stdout.write('+%%d\\n%%s' %% (len(a), a))\n    sys.stdout.flush()\n"
            % answers
        )
        return DerivedGetReader([sys.executable, str(p)], timeout=5.0)

    def test_pages_resume_from_the_cursor_and_stop_at_the_end(self, tmp_path) -> None:
        first = f"1.5\t6132\t\n1\t62\t7662\nmore\t01aabb\n"
        last = "0.5\t63\t\n"
        r = self._reader(tmp_path, [first, last])
        page = r.scan(2, ZERO, None)
        assert page.rows == [(1.5, "6132", ""), (1.0, "62", "7662")] and page.next == "01aabb"
        page = r.scan(2, ZERO, page.next)
        assert page.rows == [(0.5, "63", "")] and page.next is None
        r.close()

    def test_an_empty_group_is_an_empty_last_page(self, tmp_path) -> None:
        r = self._reader(tmp_path, [""])
        assert r.scan(3, ZERO, None) == ScanPage([], None)
        r.close()

    def test_a_malformed_row_or_cursor_is_a_read_error(self, tmp_path) -> None:
        for bad in ("nan\t61\t\n", "1\tzz\t\n", "1\t61\n", "1\t61\t\nmore\tnothex\n"):
            r = self._reader(tmp_path, [bad])
            with pytest.raises(GeoDbReadError):
                r.scan(2, ZERO, None)
            r.close()
