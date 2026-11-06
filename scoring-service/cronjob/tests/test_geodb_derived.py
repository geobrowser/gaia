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
        m = DerivedMirror(argv, timeout=0.5)
        import time

        t0 = time.monotonic()
        m.put("t", [DerivedRow(2, 0, 1.0, b"\x01", b"")])
        assert time.monotonic() - t0 < 5
        assert m.stats["errors"] == 1
        m.close()

    def test_a_child_that_never_says_ready_is_bounded(self, tmp_path) -> None:
        argv = _script(tmp_path, "import time\ntime.sleep(60)\n")
        m = DerivedMirror(argv, timeout=0.5)
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
        assert order == ["commit", "global_scores", "local_scores", "space_scores"]

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
