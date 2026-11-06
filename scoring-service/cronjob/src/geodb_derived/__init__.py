"""Best-effort mirror of the score tables into geo-db's derived class (plan 10.14, GAP-9).

Ported from gaia's Rust ``geodb-cursor`` derived mirror (``geodb-cursor/src/derived.rs``, PR
#1160): the same held ``geodb-client derived pipe`` child, the same ``put`` / ``putn`` request
lines, the same flags. Postgres stays the source of truth. The writer finishes its Postgres
transaction, then hands the rows to :class:`DerivedMirror`; nothing in here raises into the
caller. Every error is logged and counted, each request is bounded by ``timeout`` (the child is
killed when it overruns), a call is bounded by ``budget`` (the rows past it are counted as
dropped, not queued), and after a failure the child is not respawned until ``cooldown`` passes.
Rows are last-write-wins, so a dropped row is repaired by the next nightly run.

Collections (geo-db ``src/storage/derived/collection.zig``) and key layouts (plan 10.14 V10):

- ``global_scores`` (2): group 0, key the entity id (16 bytes), score the score.
- ``local_scores`` (3): group the Space id (u128), key the entity id then the Space id (32 bytes).
- ``space_scores`` (4): group 0, key the Space id (16 bytes).

None has a value. A row whose id is not a UUID, or whose score is not finite, is skipped and
counted, never truncated or coerced.

Off unless ``GEODB_DERIVED_MIRROR`` is ``1`` or ``true``; it needs ``GEODB_KEY_PATH`` and
``GEODB_BINDING_PATH``, and the key's binding must grant ``derived=2,3,4``.
"""

import logging
import math
import os
import subprocess
import threading
import time
import uuid
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass

from src.algorithm.models import Entity, Space

logger = logging.getLogger(__name__)

COLLECTION_GLOBAL = 2
COLLECTION_LOCAL = 3
COLLECTION_SPACE = 4

KEY_MAX = 48
VALUE_MAX = 64
# Requests sent between two looks at the clock.
CHUNK = 64
# Rows in one batch request (wire.derived.batch_max).
BATCH_MAX = 256

DEFAULT_TIMEOUT = 2.0
DEFAULT_COOLDOWN = 10.0
DEFAULT_BUDGET_MS = 5000


@dataclass(frozen=True)
class DerivedRow:
    """One row of a derived collection: ``(collection, key) -> (group, score, value)``."""

    collection: int
    group: int
    score: float
    key: bytes
    value: bytes = b""

    def put_line(self) -> str:
        """The pipe's request line. Raises ValueError for a row that cannot be sent."""
        if not self.key or len(self.key) > KEY_MAX:
            raise ValueError(f"key of {len(self.key)} bytes")
        if len(self.value) > VALUE_MAX:
            raise ValueError(f"value of {len(self.value)} bytes")
        if not math.isfinite(self.score):
            raise ValueError("score is not finite")
        # repr of a float is the shortest form that parses back to the same value.
        return (
            f"put\t{self.collection}\t{self.group}\t{self.score!r}"
            f"\thex:{self.key.hex()}\thex:{self.value.hex()}\n"
        )


def batch_request(lines: list[str]) -> str:
    """One batch request from ``lines`` of a single kind: ``putn<tab><n>``, then the rows.

    A pipe child older than the batch verbs would answer the head ``-`` and desync, so this is
    only sent when ``GEODB_DERIVED_BATCH`` is on.
    """
    head = "putn" if lines[0].startswith("put\t") else "deleten"
    return f"{head}\t{len(lines)}\n" + "".join(lines)


def _uuid_bytes(value: str) -> bytes:
    return uuid.UUID(str(value)).bytes


def _rows(
    items: Iterable,
    build: Callable[..., DerivedRow],
    on_skip: Callable[[Exception], None] | None,
) -> list[DerivedRow]:
    out = []
    for item in items:
        try:
            row = build(item)
            if not math.isfinite(row.score):
                raise ValueError("score is not finite")
        except (ValueError, TypeError, AttributeError) as e:
            if on_skip:
                on_skip(e)
            continue
        out.append(row)
    return out


def global_rows(entities: list[Entity], on_skip: Callable[[Exception], None] | None = None) -> list[DerivedRow]:
    """``global_scores`` (collection 2) rows for ``entities``."""
    return _rows(
        entities,
        lambda e: DerivedRow(COLLECTION_GLOBAL, 0, float(e.normalized_score), _uuid_bytes(e.id)),
        on_skip,
    )


def local_rows(entities: list[Entity], on_skip: Callable[[Exception], None] | None = None) -> list[DerivedRow]:
    """``local_scores`` (collection 3) rows for every perspective of ``entities``."""
    perspectives = (p for e in entities for p in e.perspectives)

    def build(p) -> DerivedRow:
        space = _uuid_bytes(p.space_id)
        return DerivedRow(
            COLLECTION_LOCAL,
            int.from_bytes(space, "big"),
            float(p.normalized_score),
            _uuid_bytes(p.entity_id) + space,
        )

    return _rows(perspectives, build, on_skip)


def space_rows(spaces: list[Space], on_skip: Callable[[Exception], None] | None = None) -> list[DerivedRow]:
    """``space_scores`` (collection 4) rows for ``spaces``."""
    return _rows(
        spaces,
        lambda s: DerivedRow(COLLECTION_SPACE, 0, float(s.space_score), _uuid_bytes(s.id)),
        on_skip,
    )


class _Pipe:
    def __init__(self, proc: subprocess.Popen):
        self.proc = proc


class DerivedMirror:
    """Holds the ``derived pipe`` child. Cheap to build; the child starts on first use."""

    def __init__(
        self,
        argv: list[str],
        timeout: float = DEFAULT_TIMEOUT,
        cooldown: float = DEFAULT_COOLDOWN,
        budget: float = DEFAULT_BUDGET_MS / 1000,
        batch: bool = False,
    ):
        self.argv = argv
        self.timeout = timeout
        self.cooldown = cooldown
        self.budget = budget
        self.batch = batch
        # Counters, logged by the writer after each call: puts (rows answered ok), errors
        # (failed requests), skipped (rows that cannot be sent), dropped (rows not sent).
        self.stats = {"puts": 0, "errors": 0, "skipped": 0, "dropped": 0}
        self._proc: subprocess.Popen | None = None
        self._failed_at: float | None = None

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> "DerivedMirror | None":
        """``None`` unless ``GEODB_DERIVED_MIRROR`` is on and configured."""
        env = os.environ if environ is None else environ
        if env.get("GEODB_DERIVED_MIRROR", "").strip() not in ("1", "true"):
            return None
        key, binding = env.get("GEODB_KEY_PATH"), env.get("GEODB_BINDING_PATH")
        if not key or not binding:
            logger.warning(
                "geodb_derived.misconfigured: GEODB_DERIVED_MIRROR is on but GEODB_KEY_PATH or "
                "GEODB_BINDING_PATH is unset; mirror disabled"
            )
            return None
        try:
            ms = int(env.get("GEODB_DERIVED_BUDGET_MS", "").strip() or DEFAULT_BUDGET_MS)
        except ValueError:
            ms = DEFAULT_BUDGET_MS
        return cls(
            [env.get("GEODB_CLIENT_BIN") or "geodb-client", "--key", key, "--binding", binding, "derived", "pipe"],
            budget=ms / 1000,
            batch=env.get("GEODB_DERIVED_BATCH", "").strip() in ("1", "true"),
        )

    # -- public ---------------------------------------------------------------------------

    def skipped(self, error: Exception | str, table: str = "") -> None:
        """Counts a row that cannot be mirrored."""
        self.stats["skipped"] += 1
        logger.warning("geodb_derived.row_skipped table=%s error=%s", table, error)

    def put(self, table: str, rows: list[DerivedRow]) -> None:
        """Mirrors ``rows`` of ``table`` (a label for the logs). Never raises."""
        try:
            lines = []
            for r in rows:
                try:
                    lines.append(r.put_line())
                except ValueError as e:
                    self.skipped(e, table)
            self.stats["puts"] += self._send(table, lines)
        except Exception:  # noqa: BLE001 - nothing here may fail the writer
            self.stats["errors"] += 1
            logger.exception("geodb_derived.mirror_failed table=%s; Postgres is unaffected", table)

    def close(self) -> None:
        self._drop()

    # -- internals ------------------------------------------------------------------------

    def _send(self, table: str, lines: list[str]) -> int:
        start = time.monotonic()
        sent = 0
        size = BATCH_MAX if self.batch else CHUNK
        for i in range(0, len(lines), size):
            chunk = lines[i : i + size]
            if sent > 0 and time.monotonic() - start > self.budget:
                dropped = len(lines) - sent
                self.stats["dropped"] += dropped
                logger.warning(
                    "geodb_derived.budget_spent table=%s dropped=%d; the rest is left for the next run",
                    table,
                    dropped,
                )
                break
            reqs = [batch_request(chunk)] if self.batch and len(chunk) > 1 else chunk
            try:
                self._request_all(reqs)
            except _CoolingDown:
                self.stats["errors"] += 1
                self.stats["dropped"] += len(lines) - sent
                break
            except Exception as e:  # noqa: BLE001
                logger.warning("geodb_derived.mirror_failed table=%s error=%s; Postgres is unaffected", table, e)
                self.stats["errors"] += 1
                self.stats["dropped"] += len(lines) - sent
                break
            sent += len(chunk)
        return sent

    def _request_all(self, reqs: list[str]) -> None:
        """Sends requests in order over the held child; the first failure drops it."""
        proc = self._ensure()
        for req in reqs:
            timer = threading.Timer(self.timeout, proc.kill)
            timer.start()
            try:
                self._request(proc, req)
            except Exception:
                self._drop()
                self._failed_at = time.monotonic()
                raise
            finally:
                timer.cancel()
        self._failed_at = None

    def _ensure(self) -> subprocess.Popen:
        if self._proc is not None:
            return self._proc
        if self._failed_at is not None and time.monotonic() - self._failed_at < self.cooldown:
            raise _CoolingDown()
        try:
            proc = subprocess.Popen(
                self.argv,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
        except OSError:
            self._failed_at = time.monotonic()
            raise
        timer = threading.Timer(self.timeout, proc.kill)
        timer.start()
        try:
            first = proc.stdout.readline()
        finally:
            timer.cancel()
        if first.rstrip(b"\r\n") != b"ready":
            proc.kill()
            proc.wait()
            self._failed_at = time.monotonic()
            raise RuntimeError(f"pipe not ready: {first!r}")
        self._proc = proc
        return proc

    @staticmethod
    def _request(proc: subprocess.Popen, req: str) -> None:
        """One request line (or batch), one frame: ``+<n>\\n<n bytes>`` ok, ``-<n>`` refused."""
        proc.stdin.write(req.encode())
        proc.stdin.flush()
        head = proc.stdout.readline()
        if not head:
            raise RuntimeError("pipe closed")
        head = head.rstrip(b"\r\n").decode(errors="replace")
        sign, n = head[:1], head[1:]
        if not n.isdigit() or int(n) > 4096:
            raise RuntimeError(f"bad frame {head!r}")
        body = proc.stdout.read(int(n))
        if len(body) != int(n):
            raise RuntimeError("pipe closed mid-frame")
        if sign == "+":
            return
        if sign == "-":
            raise RuntimeError(f"refused: {body.decode(errors='replace')}")
        raise RuntimeError(f"bad frame {head!r}")

    def _drop(self) -> None:
        proc, self._proc = self._proc, None
        if proc is None:
            return
        try:
            proc.kill()
            proc.wait(timeout=5)
        except Exception:  # noqa: BLE001
            pass


class _CoolingDown(Exception):
    pass
