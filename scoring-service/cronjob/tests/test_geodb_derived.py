"""Tests for the best-effort geo-db derived-class mirror of the score tables."""

import logging
import math
import stat
import sys
import uuid
from datetime import datetime
from unittest.mock import MagicMock, patch

import pytest

from src.algorithm.models import Entity, Perspective, Space
from src.geodb_derived import (
    BATCH_MAX,
    CHUNK,
    COLLECTION_GLOBAL,
    COLLECTION_LOCAL,
    COLLECTION_SPACE,
    DerivedMirror,
    DerivedRow,
    batch_request,
    global_rows,
    local_rows,
    space_rows,
)
from src.scoring_data_writer import ScoringDataWriter

E1 = "11111111-1111-4111-8111-111111111111"
E2 = "22222222-2222-4222-8222-222222222222"
S1 = "33333333-3333-4333-8333-333333333333"


def _script(tmp_path, body: str) -> list[str]:
    """A fake geodb-client: a python script run as the pipe child."""
    p = tmp_path / "fake.py"
    p.write_text(body)
    return [sys.executable, str(p)]


def _logging_child(tmp_path, answer: str = "+10\\napplied: 1") -> tuple[list[str], "object"]:
    log = tmp_path / "log.txt"
    body = f"""
import sys
print("ready", flush=True)
for l in sys.stdin:
    open({str(log)!r}, "a").write(l)
    if l.startswith(("putn\\t", "deleten\\t")):
        n = int(l.split("\\t")[1])
        for _ in range(n):
            open({str(log)!r}, "a").write(sys.stdin.readline())
    sys.stdout.write("{answer}")
    sys.stdout.flush()
"""
    return _script(tmp_path, body), log


def entity(eid: str, score: float, perspectives=()) -> Entity:
    e = Entity(id=eid, created_at=datetime.now())
    e.normalized_score = score
    e.perspectives = list(perspectives)
    return e


def persp(eid: str, sid: str, score: float) -> Perspective:
    p = Perspective(id=f"{eid}_{sid}", entity_id=eid, space_id=sid, created_at=datetime.now())
    p.normalized_score = score
    return p


class TestKeyLayouts:
    def test_global_scores_collection_2(self) -> None:
        rows = global_rows([entity(E1, 0.5)])
        assert rows == [
            DerivedRow(collection=2, group=0, score=0.5, key=uuid.UUID(E1).bytes, value=b"")
        ]
        assert COLLECTION_GLOBAL == 2

    def test_local_scores_collection_3_group_is_space(self) -> None:
        rows = local_rows([entity(E1, 0.1, [persp(E1, S1, 0.75)])])
        assert len(rows) == 1
        r = rows[0]
        assert r.collection == COLLECTION_LOCAL == 3
        assert r.group == int.from_bytes(uuid.UUID(S1).bytes, "big")
        assert r.key == uuid.UUID(E1).bytes + uuid.UUID(S1).bytes
        assert len(r.key) == 32
        assert r.score == 0.75

    def test_space_scores_collection_4(self) -> None:
        s = Space(id=S1, created_at=datetime.now())
        s.space_score = 1.25
        rows = space_rows([s])
        assert rows == [
            DerivedRow(collection=4, group=0, score=1.25, key=uuid.UUID(S1).bytes, value=b"")
        ]
        assert COLLECTION_SPACE == 4

    def test_undashed_ids_are_accepted(self) -> None:
        assert global_rows([entity(E1.replace("-", ""), 1.0)])[0].key == uuid.UUID(E1).bytes

    def test_bad_id_or_score_is_skipped_and_counted(self) -> None:
        m = DerivedMirror(["/nonexistent"])
        rows = global_rows(
            [entity("entity-1", 1.0), entity(E1, math.nan), entity(E2, math.inf), entity(E2, 2.0)],
            on_skip=m.skipped,
        )
        assert [r.score for r in rows] == [2.0]
        assert m.stats["skipped"] == 3


class TestPutLine:
    def test_put_line_is_hex_with_shortest_float(self) -> None:
        r = DerivedRow(2, 0, 0.5, bytes([1] * 16), b"")
        assert r.put_line() == f"put\t2\t0\t0.5\thex:{'01' * 16}\thex:\n"

    def test_group_is_decimal_u128(self) -> None:
        r = DerivedRow(3, 171, 1.0, b"\x01", b"")
        assert r.put_line().startswith("put\t3\t171\t1.0\t")

    @pytest.mark.parametrize("key", [b"", bytes(49)])
    def test_bad_key_refused(self, key: bytes) -> None:
        with pytest.raises(ValueError):
            DerivedRow(2, 0, 1.0, key, b"").put_line()

    def test_non_finite_refused(self) -> None:
        with pytest.raises(ValueError):
            DerivedRow(2, 0, math.nan, b"\x01", b"").put_line()

    def test_batch_request_is_head_then_rows(self) -> None:
        lines = ["put\t2\t0\t1.0\thex:aa\thex:\n", "put\t2\t0\t2.0\thex:bb\thex:\n"]
        assert batch_request(lines) == "putn\t2\n" + "".join(lines)


class TestFlag:
    def test_off_by_default(self) -> None:
        assert DerivedMirror.from_env({}) is None

    def test_cursor_flag_does_not_turn_it_on(self) -> None:
        env = {"GEODB_CURSOR_MIRROR": "1", "GEODB_KEY_PATH": "/k", "GEODB_BINDING_PATH": "/b"}
        assert DerivedMirror.from_env(env) is None

    @pytest.mark.parametrize("v", ["0", "", "false", "off"])
    def test_other_values_are_off(self, v: str) -> None:
        env = {"GEODB_DERIVED_MIRROR": v, "GEODB_KEY_PATH": "/k", "GEODB_BINDING_PATH": "/b"}
        assert DerivedMirror.from_env(env) is None

    def test_on_without_paths_is_disabled(self, caplog) -> None:
        with caplog.at_level(logging.WARNING):
            assert DerivedMirror.from_env({"GEODB_DERIVED_MIRROR": "1"}) is None
        assert "misconfigured" in caplog.text

    @pytest.mark.parametrize("v", ["1", "true"])
    def test_on(self, v: str) -> None:
        env = {
            "GEODB_DERIVED_MIRROR": v,
            "GEODB_KEY_PATH": "/k",
            "GEODB_BINDING_PATH": "/b",
            "GEODB_CLIENT_BIN": "/geodb/geodb-client",
            "GEODB_DERIVED_BUDGET_MS": "1500",
            "GEODB_DERIVED_BATCH": "1",
        }
        m = DerivedMirror.from_env(env)
        assert m is not None
        assert m.argv == ["/geodb/geodb-client", "--key", "/k", "--binding", "/b", "derived", "pipe"]
        assert m.budget == 1.5
        assert m.batch is True


class TestTableSelection:
    ENV = {"GEODB_DERIVED_MIRROR": "1", "GEODB_KEY_PATH": "/k", "GEODB_BINDING_PATH": "/b"}

    def test_all_tables_by_default(self) -> None:
        for extra in ({}, {"GEODB_DERIVED_MIRROR_TABLES": ""}, {"GEODB_DERIVED_MIRROR_TABLES": " , "}):
            m = DerivedMirror.from_env({**self.ENV, **extra})
            assert m is not None
            assert all(m.enabled(t) for t in ("global_scores", "local_scores", "space_scores"))

    def test_subset(self) -> None:
        m = DerivedMirror.from_env({**self.ENV, "GEODB_DERIVED_MIRROR_TABLES": "global_scores, space_scores"})
        assert m is not None
        assert m.enabled("global_scores") and m.enabled("space_scores") and not m.enabled("local_scores")

    def test_unknown_name_is_ignored_and_none_valid_disables(self, caplog) -> None:
        with caplog.at_level(logging.WARNING):
            m = DerivedMirror.from_env({**self.ENV, "GEODB_DERIVED_MIRROR_TABLES": "global_scores,bogus"})
            assert m is not None and m.enabled("global_scores") and not m.enabled("bogus")
            assert DerivedMirror.from_env({**self.ENV, "GEODB_DERIVED_MIRROR_TABLES": "bogus"}) is None
        assert "unknown table" in caplog.text

    def test_writer_skips_unselected_tables(self) -> None:
        mirror = DerivedMirror(["x"], tables=("global_scores", "space_scores"))
        calls: list[str] = []
        mirror.put = lambda table, rows: calls.append(table)  # type: ignore[method-assign]
        w = ScoringDataWriter("postgresql://x", derived=mirror)
        s = Space(id=S1, created_at=datetime.now())
        s.space_score = 2.0
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as c:
            c.return_value.__enter__.return_value = MagicMock()
            w.write_all([entity(E1, 0.5, [persp(E1, S1, 0.7)])], [s])
        assert calls == ["global_scores", "space_scores"]


class TestSending:
    def test_rows_go_in_order_one_line_each(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5)
        m.put("global_scores", global_rows([entity(E1, 0.5), entity(E2, 1.5)]))
        sent = log.read_text()
        assert sent == (
            f"put\t2\t0\t0.5\thex:{uuid.UUID(E1).hex}\thex:\n"
            f"put\t2\t0\t1.5\thex:{uuid.UUID(E2).hex}\thex:\n"
        )
        assert m.stats["puts"] == 2
        assert m.stats["errors"] == 0
        m.close()

    def test_batching_sends_256_rows_a_request(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True)
        rows = [DerivedRow(2, 0, 1.0, i.to_bytes(2, "big"), b"") for i in range(BATCH_MAX + 10)]
        m.put("global_scores", rows)
        heads = [l for l in log.read_text().splitlines() if l.startswith("putn")]
        assert heads == ["putn\t256", "putn\t10"]
        assert m.stats["puts"] == BATCH_MAX + 10
        m.close()

    def test_a_single_row_in_batch_mode_is_a_plain_put(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True)
        m.put("t", [DerivedRow(2, 0, 1.0, b"\x01", b"")])
        assert log.read_text().startswith("put\t2")
        m.close()

    def test_refusal_is_counted_logged_and_does_not_raise(self, tmp_path, caplog) -> None:
        argv = _script(
            tmp_path,
            'import sys\nprint("ready", flush=True)\nsys.stdin.readline()\n'
            'sys.stdout.write("-26\\nrefused: DefiniteRefusal X")\nsys.stdout.flush()\n',
        )
        m = DerivedMirror(argv, timeout=5)
        rows = [DerivedRow(2, 0, 1.0, bytes([i]), b"") for i in range(1, 4)]
        with caplog.at_level(logging.WARNING):
            m.put("global_scores", rows)  # must not raise
        assert m.stats["errors"] == 1
        assert m.stats["puts"] == 0
        assert m.stats["dropped"] == 3
        assert "mirror_failed" in caplog.text
        # cooling down: the next call sends nothing and still returns
        m.put("global_scores", rows)
        assert m.stats["errors"] == 2
        m.close()

    def test_missing_binary_is_swallowed(self) -> None:
        m = DerivedMirror(["/nonexistent/geodb-client"], timeout=1, cooldown=0)
        m.put("t", [DerivedRow(2, 0, 1.0, b"\x01", b"")])
        assert m.stats["errors"] == 1

    def test_a_hung_child_is_bounded_by_the_timeout(self, tmp_path) -> None:
        argv = _script(tmp_path, 'import time\nprint("ready", flush=True)\ntime.sleep(60)\n')
        m = DerivedMirror(argv, timeout=0.5, retries=0)
        import time

        t0 = time.monotonic()
        m.put("t", [DerivedRow(2, 0, 1.0, b"\x01", b"")])
        assert time.monotonic() - t0 < 5
        assert m.stats["errors"] == 1
        m.close()

    def test_a_child_that_never_says_ready_is_bounded(self, tmp_path) -> None:
        argv = _script(tmp_path, "import time\ntime.sleep(60)\n")
        m = DerivedMirror(argv, timeout=0.5, retries=0)
        import time

        t0 = time.monotonic()
        m.put("t", [DerivedRow(2, 0, 1.0, b"\x01", b"")])
        assert time.monotonic() - t0 < 5
        assert m.stats["errors"] == 1

    def test_spent_budget_leaves_the_rest_unsent(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, budget=0.0)
        rows = [DerivedRow(2, 0, 1.0, i.to_bytes(2, "big"), b"") for i in range(100)]
        m.put("t", rows)
        assert len(log.read_text().splitlines()) == 64  # first chunk only
        assert m.stats["dropped"] == 36
        m.close()

    def test_bad_rows_skipped_good_rows_sent(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5)
        m.put("t", [DerivedRow(2, 0, math.nan, b"\x01", b""), DerivedRow(2, 0, 1.0, b"\x02", b"")])
        assert len(log.read_text().splitlines()) == 1
        assert m.stats["skipped"] == 1
        assert m.stats["puts"] == 1
        m.close()


class TestWriterIntegration:
    def _writer(self, mirror):
        return ScoringDataWriter("postgresql://x", derived=mirror)

    def _run(self, writer, entities, spaces):
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as c:
            c.return_value.__enter__.return_value = MagicMock()
            writer.write_all(entities, spaces)

    def test_flag_off_makes_no_geodb_calls(self) -> None:
        w = ScoringDataWriter("postgresql://x")
        assert w._derived is None
        with patch("src.geodb_derived.subprocess.Popen") as popen:
            self._run(w, [entity(E1, 0.5)], [])
            popen.assert_not_called()

    def test_rows_are_mirrored_after_postgres_commit(self) -> None:
        order: list[str] = []
        mirror = MagicMock()
        mirror.stats = {"puts": 0, "errors": 0, "skipped": 0, "dropped": 0}
        mirror.skipped = MagicMock()
        mirror.put.side_effect = lambda table, rows: order.append(table)
        w = self._writer(mirror)
        s = Space(id=S1, created_at=datetime.now())
        s.space_score = 2.0
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as c:
            conn = MagicMock()
            c.return_value.__enter__.return_value = conn
            conn.transaction.return_value.__exit__.side_effect = lambda *a: order.append("commit")
            w.write_all([entity(E1, 0.5, [persp(E1, S1, 0.7)])], [s])
        # The savepoints around the state writes also exit as "commit"; the outer one is the last before the puts, and the final state write adds one after them.
        assert order[-5:-1] == ["commit", "global_scores", "local_scores", "space_scores"]

    def test_geodb_failure_does_not_fail_the_write(self) -> None:
        mirror = MagicMock()
        mirror.put.side_effect = RuntimeError("geo-db exploded")
        w = self._writer(mirror)
        self._run(w, [entity(E1, 0.5)], [])  # must not raise

    def test_postgres_failure_sends_nothing_to_geodb(self) -> None:
        mirror = MagicMock()
        w = self._writer(mirror)
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as c:
            c.side_effect = RuntimeError("pg down")
            with pytest.raises(RuntimeError):
                w.write_all([entity(E1, 0.5)], [])
        mirror.put.assert_not_called()


class _StatMirror:
    """A mirror double with real counters; ``put`` records the rows and can fake a drop."""

    def __init__(self, drop: str | None = None) -> None:
        self.stats = {"puts": 0, "errors": 0, "skipped": 0, "dropped": 0}
        self.sent: dict[str, list[DerivedRow]] = {}
        self.drop = drop

    def enabled(self, table: str) -> bool:
        return True

    def skipped(self, error, table: str = "") -> None:
        self.stats["skipped"] += 1

    def put(self, table: str, rows) -> None:
        rows = list(rows)
        self.sent[table] = rows
        if table == self.drop:
            self.stats["dropped"] += len(rows)
        else:
            self.stats["puts"] += len(rows)


E3 = "44444444-4444-4444-8444-444444444444"
S2 = "55555555-5555-4555-8555-555555555555"


class TestChangedOnlyMirror:
    def _data(self):
        ents = [
            entity(E1, 0.5, [persp(E1, S1, 0.7), persp(E1, S2, 0.8)]),
            entity(E2, 0.6, [persp(E2, S1, 0.9)]),
            entity(E3, 0.1),
        ]
        sp = Space(id=S1, created_at=datetime.now())
        sp.space_score = 2.0
        return ents, [sp]

    def _write(self, mirror, synced, changed):
        w = ScoringDataWriter("postgresql://x", derived=mirror)
        ents, sp = self._data()
        marks: list[tuple[list[str], bool]] = []
        with (
            patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as c,
            patch.object(ScoringDataWriter, "_synced_tables", return_value=set(synced)),
            patch.object(ScoringDataWriter, "_changed_keys", side_effect=lambda conn, t, e: changed[t]),
            patch.object(ScoringDataWriter, "_set_synced", side_effect=lambda conn, t, v: marks.append((list(t), v))),
        ):
            c.return_value.__enter__.return_value = MagicMock()
            w.write_all(ents, sp)
        return marks

    def test_unsynced_tables_are_sent_whole(self) -> None:
        m = _StatMirror()
        marks = self._write(m, set(), {})
        assert len(m.sent["global_scores"]) == 3
        assert len(m.sent["local_scores"]) == 3
        assert len(m.sent["space_scores"]) == 1
        # Marked unsynced inside the write, synced after a clean mirror.
        assert marks == [(["global_scores", "local_scores", "space_scores"], False),
                         (["global_scores", "local_scores", "space_scores"], True)]

    def test_synced_tables_send_only_changed_keys(self) -> None:
        m = _StatMirror()
        changed = {"global_scores": {E2}, "local_scores": {(E1, S2)}}
        self._write(m, {"global_scores", "local_scores"}, changed)
        assert [r.key for r in m.sent["global_scores"]] == [uuid.UUID(E2).bytes]
        assert [r.key for r in m.sent["local_scores"]] == [uuid.UUID(E1).bytes + uuid.UUID(S2).bytes]
        assert len(m.sent["space_scores"]) == 1  # small table: always whole

    def test_nothing_changed_sends_nothing(self) -> None:
        m = _StatMirror()
        self._write(m, {"global_scores", "local_scores"}, {"global_scores": set(), "local_scores": set()})
        assert m.sent["global_scores"] == [] and m.sent["local_scores"] == []

    def test_a_dropped_table_stays_unsynced(self) -> None:
        m = _StatMirror(drop="local_scores")
        marks = self._write(m, set(), {})
        assert marks[-1] == (["global_scores", "space_scores"], True)

    def test_unsynced_state_is_not_marked_when_mirror_is_off(self) -> None:
        marks = self._write(MagicMock(**{"enabled.return_value": False, "stats": {"puts": 0, "errors": 0, "skipped": 0, "dropped": 0}}), set(), {})
        assert all(t == [] for t, _ in marks)


class TestLazySend:
    def test_budget_drop_counts_the_unsent_rest_of_a_generator(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, budget=0.0)
        rows = (DerivedRow(COLLECTION_GLOBAL, 0, 1.0, uuid.UUID(int=i + 1).bytes) for i in range(500))
        m.put("global_scores", rows)
        # One chunk goes (the budget is only looked at after the first), the other 436 are dropped.
        assert m.stats["puts"] == 64
        assert m.stats["dropped"] == 436
        m.close()

    def test_local_rows_only_filters_perspectives(self) -> None:
        e = entity(E1, 0.5, [persp(E1, S1, 0.7), persp(E1, S2, 0.8)])
        rows = local_rows([e], only={(E1, S2)})
        assert [r.key for r in rows] == [uuid.UUID(E1).bytes + uuid.UUID(S2).bytes]


class TestSessionExpiry:
    """The serve's grant lives 300 s: a long table outlives one session (first #1336 nightly)."""

    @staticmethod
    def _child(tmp_path, die_after: int, how: str):
        """A fake pipe child. The first ``die_after`` launches fail on their 3rd request (``how``:
        ``expire`` answers CapabilityTokenExpired, ``close`` exits); later launches are healthy."""
        launches = tmp_path / "launches.txt"
        log = tmp_path / "log.txt"
        body = f"""
import os, sys
launches = {str(launches)!r}
open(launches, "a").write("x")
mine = len(open(launches).read())
print("ready", flush=True)
n = 0
for l in sys.stdin:
    n += 1
    if mine <= {die_after} and n == 3:
        if {how!r} == "close":
            sys.exit(0)
        msg = "refused: DefiniteRefusal CapabilityTokenExpired"
        sys.stdout.write("-%d\\n%s" % (len(msg), msg))
        sys.stdout.flush()
        continue
    open({str(log)!r}, "a").write(l)
    sys.stdout.write("+0\\n")
    sys.stdout.flush()
"""
        return _script(tmp_path, body), launches, log

    @staticmethod
    def _rows(n: int):
        return [DerivedRow(3, 0, 1.0, i.to_bytes(3, "big"), b"") for i in range(n)]

    @pytest.mark.parametrize("how", ["expire", "close"])
    def test_a_lost_session_is_replaced_and_the_same_table_resumes(self, tmp_path, how) -> None:
        argv, launches, log = self._child(tmp_path, die_after=1, how=how)
        m = DerivedMirror(argv, timeout=5, retry_delay=0)
        m.put("local_scores", self._rows(CHUNK * 4))  # 256 plain puts, 4 chunks of 64
        # Every row arrived. The failed chunk is resent whole, so the rows its first session had
        # already answered arrive twice (puts are last-write-wins); none is lost.
        lines = log.read_text().splitlines()
        assert len(set(lines)) == CHUNK * 4
        assert len(lines) == CHUNK * 4 + 2
        assert m.stats["errors"] == 0
        assert m.stats["dropped"] == 0
        assert m.stats["puts"] == CHUNK * 4
        assert m.stats["reconnects"] == 1
        assert len(launches.read_text()) == 2
        m.close()

    def test_every_row_is_delivered_after_an_expiry(self, tmp_path) -> None:
        argv, _, log = self._child(tmp_path, die_after=1, how="expire")
        m = DerivedMirror(argv, timeout=5, retry_delay=0)
        rows = self._rows(10)
        m.put("local_scores", rows)
        got = {l.split("\t")[4] for l in log.read_text().splitlines()}
        assert got == {"hex:" + r.key.hex() for r in rows}
        m.close()

    def test_retries_are_bounded(self, tmp_path, caplog) -> None:
        argv, launches, log = self._child(tmp_path, die_after=99, how="expire")
        m = DerivedMirror(argv, timeout=5, retries=2, retry_delay=0)
        with caplog.at_level(logging.WARNING):
            m.put("local_scores", self._rows(CHUNK * 4))
        assert m.stats["errors"] == 1
        assert m.stats["reconnects"] == 2
        assert len(launches.read_text()) == 3  # the first session and two resends
        assert m.stats["puts"] == 0  # puts count whole chunks; the first never completed
        assert m.stats["dropped"] == CHUNK * 4  # the failed chunk onward
        assert "mirror_failed" in caplog.text
        m.close()

    def test_other_refusals_are_not_retried(self, tmp_path) -> None:
        argv = _script(
            tmp_path,
            'import sys\nprint("ready", flush=True)\nsys.stdin.readline()\n'
            'sys.stdout.write("-26\\nrefused: DefiniteRefusal X")\nsys.stdout.flush()\n',
        )
        m = DerivedMirror(argv, timeout=5, retry_delay=0)
        m.put("t", self._rows(3))
        assert m.stats["errors"] == 1
        assert m.stats["reconnects"] == 0

    def test_a_session_past_its_age_is_replaced_before_it_can_expire(self, tmp_path) -> None:
        argv, launches, log = self._child(tmp_path, die_after=0, how="expire")
        m = DerivedMirror(argv, timeout=5, session_refresh=0.0, retry_delay=0)
        m.put("local_scores", self._rows(CHUNK * 3))
        assert len(launches.read_text()) == 3  # one session a chunk
        assert m.stats["reconnects"] == 0  # a refresh is not a failure
        assert m.stats["errors"] == 0
        assert m.stats["puts"] == CHUNK * 3
        m.close()

    def test_a_fresh_session_is_kept_while_young(self, tmp_path) -> None:
        argv, launches, _ = self._child(tmp_path, die_after=0, how="expire")
        m = DerivedMirror(argv, timeout=5, session_refresh=3600.0)
        m.put("local_scores", self._rows(CHUNK * 3))
        assert len(launches.read_text()) == 1
        m.close()

    def test_a_resend_ignores_the_cooldown(self, tmp_path) -> None:
        argv, launches, _ = self._child(tmp_path, die_after=1, how="close")
        m = DerivedMirror(argv, timeout=5, cooldown=3600, retry_delay=0)
        m.put("t", self._rows(CHUNK * 2))
        assert m.stats["errors"] == 0
        assert m.stats["puts"] == CHUNK * 2
        m.close()

    def test_env_sets_refresh_and_retries(self, tmp_path) -> None:
        env = {
            "GEODB_DERIVED_MIRROR": "1",
            "GEODB_KEY_PATH": "/k",
            "GEODB_BINDING_PATH": "/b",
            "GEODB_DERIVED_SESSION_REFRESH_S": "120",
            "GEODB_DERIVED_RETRIES": "5",
        }
        m = DerivedMirror.from_env(env)
        assert (m.session_refresh, m.retries) == (120.0, 5)
        env["GEODB_DERIVED_RETRIES"] = "junk"
        del env["GEODB_DERIVED_SESSION_REFRESH_S"]
        m = DerivedMirror.from_env(env)
        assert (m.session_refresh, m.retries) == (240.0, 3)


def _env(**extra) -> dict:
    return {"GEODB_DERIVED_MIRROR": "1", "GEODB_KEY_PATH": "k", "GEODB_BINDING_PATH": "b", **extra}


def _rows(n: int) -> list[DerivedRow]:
    return [DerivedRow(2, 0, 1.0, i.to_bytes(2, "big"), b"") for i in range(n)]


class TestPacing:
    def test_default_is_unpaced(self) -> None:
        m = DerivedMirror.from_env(_env())
        assert m.max_rows_per_s == 0.0
        assert m.batch_rows == BATCH_MAX

    def test_env_sets_rate_and_batch_rows(self) -> None:
        m = DerivedMirror.from_env(_env(GEODB_DERIVED_MAX_ROWS_PER_S="200", GEODB_DERIVED_BATCH_ROWS="32"))
        assert m.max_rows_per_s == 200.0
        assert m.batch_rows == 32

    @pytest.mark.parametrize("raw", ["", "abc", "-5", "0", "nan", "inf"])
    def test_bad_rate_means_unlimited(self, raw) -> None:
        assert DerivedMirror.from_env(_env(GEODB_DERIVED_MAX_ROWS_PER_S=raw)).max_rows_per_s == 0.0

    def test_batch_rows_is_clamped(self) -> None:
        assert DerivedMirror(["x"], batch_rows=0).batch_rows == 1
        assert DerivedMirror(["x"], batch_rows=10_000).batch_rows == BATCH_MAX

    def test_smaller_batches_when_set(self, tmp_path) -> None:
        argv, log = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True, batch_rows=40)
        m.put("t", _rows(100))
        heads = [l for l in log.read_text().splitlines() if l.startswith("putn")]
        assert heads == ["putn\t40", "putn\t40", "putn\t20"]
        m.close()

    def test_unpaced_never_sleeps(self, tmp_path) -> None:
        argv, _ = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True)
        with patch("src.geodb_derived.time.sleep") as sleep:
            m.put("t", _rows(600))
        sleep.assert_not_called()
        m.close()

    def test_each_batch_takes_at_least_rows_over_rate(self, tmp_path) -> None:
        argv, _ = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True, batch_rows=100, max_rows_per_s=1000.0)
        waits: list[float] = []
        with patch("src.geodb_derived.time.sleep", side_effect=waits.append):
            m.put("t", _rows(250))
        # 100, 100 and 50 rows at 1000 rows/s: 0.1 s, 0.1 s, 0.05 s, less the real send time.
        assert len(waits) == 3
        assert all(0 < w <= 0.1 for w in waits[:2]) and 0 < waits[2] <= 0.05
        assert m.stats["puts"] == 250
        m.close()

    def test_a_slow_answer_gets_no_extra_wait(self, tmp_path) -> None:
        argv, _ = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True, batch_rows=100, max_rows_per_s=1e9)
        with patch("src.geodb_derived.time.sleep") as sleep:
            m.put("t", _rows(100))
        sleep.assert_not_called()  # the send already took longer than 100 / 1e9 s
        m.close()

    def test_paced_wait_is_not_counted_against_the_budget(self, tmp_path) -> None:
        argv, _ = _logging_child(tmp_path)
        m = DerivedMirror(argv, timeout=5, batch=True, batch_rows=50, max_rows_per_s=100.0, budget=0.6)
        # Real sleeps: 4 batches of 50 rows at 100 rows/s take about 2 s, over the 0.6 s budget.
        m.put("t", _rows(200))
        assert m.stats["dropped"] == 0
        assert m.stats["puts"] == 200
        m.close()
