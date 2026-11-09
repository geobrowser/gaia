"""Best-effort mirror of the score tables into geo-db's derived class (plan 10.14, GAP-9).

Ported from gaia's Rust ``geodb-cursor`` derived mirror (``geodb-cursor/src/derived.rs``, PR
#1160): the same held ``geodb-client derived pipe`` child, the same ``put`` / ``putn`` request
lines, the same flags. Postgres stays the source of truth. The writer finishes its Postgres
transaction, then hands the rows to :class:`DerivedMirror`; nothing in here raises into the
caller. Every error is logged and counted, each request is bounded by ``timeout`` (the child is
killed when it overruns), a call is bounded by ``budget`` (the rows past it are counted as
dropped, not queued), and after a failure the child is not respawned until ``cooldown`` passes.
Rows are last-write-wins. The writer sends only the rows whose score changed since the last run
that mirrored every row it tried (``score_mirror_state``); after any drop or error the next run
rewrites the whole table, so the mirror converges instead of relying on a full rewrite fitting in
one budget.

Collections (geo-db ``src/storage/derived/collection.zig``) and key layouts (plan 10.14 V10):

- ``global_scores`` (2): group 0, key the entity id (16 bytes), score the score.
- ``local_scores`` (3): group the Space id (u128), key the entity id then the Space id (32 bytes).
- ``space_scores`` (4): group 0, key the Space id (16 bytes).

None has a value. A row whose id is not a UUID, or whose score is not finite, is skipped and
counted, never truncated or coerced.

Off unless ``GEODB_DERIVED_MIRROR`` is ``1`` or ``true``; it needs ``GEODB_KEY_PATH`` and
``GEODB_BINDING_PATH``, and the key's binding must grant ``derived=2,3,4``.

Session lifetime. Each child holds one geo-db session whose capability grant the serve mints for
``grant_ttl_s`` (300 s on the testnet serve, ``geodb/supervisor.sh``); a request past it is refused
``CapabilityTokenExpired``. A full ``local_scores`` rewrite takes longer than that, so the mirror
(1) restarts the child, a fresh handshake and grant, once its session is older than
``GEODB_DERIVED_SESSION_REFRESH_S`` (default 240) and (2) on a lost pipe or an expired token
re-handshakes and resends the same chunk, up to ``GEODB_DERIVED_RETRIES`` times (default 3) a chunk
and ``MAX_RECONNECTS`` a call. Puts are last-write-wins, so a resend is safe. Any other refusal still
ends the table.

``GEODB_DERIVED_MIRROR_TABLES`` (comma separated, e.g. ``global_scores,space_scores``) picks which tables are
mirrored. Unset or empty mirrors all three, as before. A name that is not one of the three is ignored with a
warning; if none is valid, nothing is mirrored (the cap is a reason to fail closed).

Pacing. The held pipe sends one request at a time and waits for its answer, so the mirror already
runs as fast as geo-db answers, and a full table rewrite keeps the serve's write path busy for as
long as it lasts. Two knobs, both off by default (behaviour is unchanged unless set):
``GEODB_DERIVED_MAX_ROWS_PER_S`` (a number; 0 or unset is unlimited) makes each request take at least
``rows / rate`` seconds, with no catching up after a slow one, and ``GEODB_DERIVED_BATCH_ROWS`` (1 to
256; default 256) sets the rows in a batch request, so a smaller write holds the serve for less
time. Either may be set for one table with a suffix, e.g. ``GEODB_DERIVED_MAX_ROWS_PER_S_GLOBAL_SCORES``
(the table name upper-cased), which wins over the plain name for that table. Time spent waiting for the pace is not counted against ``GEODB_DERIVED_BUDGET_MS``: the budget
bounds the time spent sending, so a slower pace does not drop the rows a faster one would have sent.
"""

import logging
import math
import os
import subprocess
import threading
import time
import uuid
from itertools import islice
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

TABLES = ("global_scores", "local_scores", "space_scores")

DEFAULT_TIMEOUT = 2.0
DEFAULT_COOLDOWN = 10.0
DEFAULT_BUDGET_MS = 5000
# The serve's grant lives 300 s (GEODB_GRANT_TTL_S); a session is replaced well before that.
DEFAULT_SESSION_REFRESH_S = 240.0
# Re-handshakes a chunk may take before its table is given up, and the most one call may make.
DEFAULT_RETRIES = 3
MAX_RECONNECTS = 20
DEFAULT_RETRY_DELAY = 1.0


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


def norm_id(value) -> str:
    """A uuid as lowercase dashed text, whether it arrives as text or a uuid; other text is kept as is."""
    try:
        return str(uuid.UUID(str(value)))
    except ValueError:
        return str(value)


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


def local_rows(
    entities: list[Entity],
    on_skip: Callable[[Exception], None] | None = None,
    only: set | None = None,
) -> list[DerivedRow]:
    """``local_scores`` (collection 3) rows for every perspective of ``entities``.

    With ``only`` (a set of ``(norm_id(entity), norm_id(space))``) just those perspectives.
    """
    perspectives = (
        p
        for e in entities
        for p in e.perspectives
        if only is None or (norm_id(p.entity_id), norm_id(p.space_id)) in only
    )

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


def _parse_tables(raw: str) -> tuple[str, ...]:
    """The tables named in ``raw``; every table when it is blank. Unknown names are warned about and dropped."""
    names = [n.strip() for n in raw.split(",") if n.strip()]
    if not names:
        return TABLES
    for n in names:
        if n not in TABLES:
            logger.warning("geodb_derived.misconfigured: unknown table %r in GEODB_DERIVED_MIRROR_TABLES; ignored", n)
    return tuple(t for t in TABLES if t in names)


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
        tables: Iterable[str] = TABLES,
        session_refresh: float = DEFAULT_SESSION_REFRESH_S,
        retries: int = DEFAULT_RETRIES,
        retry_delay: float = DEFAULT_RETRY_DELAY,
        max_rows_per_s: float = 0.0,
        batch_rows: int = BATCH_MAX,
        table_rates: Mapping[str, float] | None = None,
        table_batch_rows: Mapping[str, int] | None = None,
    ):
        self.argv = argv
        self.max_rows_per_s = _rate(max_rows_per_s)
        self.batch_rows = _batch_rows(batch_rows)
        self.table_rates = {t: _rate(v) for t, v in (table_rates or {}).items()}
        self.table_batch_rows = {t: _batch_rows(v) for t, v in (table_batch_rows or {}).items()}
        self.session_refresh = session_refresh
        self.retries = retries
        self.retry_delay = retry_delay
        self.timeout = timeout
        self.cooldown = cooldown
        self.budget = budget
        self.batch = batch
        self.tables = frozenset(tables)
        # Counters, logged by the writer after each call: puts (rows answered ok), errors
        # (failed requests), skipped (rows that cannot be sent), dropped (rows not sent).
        self.stats = {"puts": 0, "errors": 0, "skipped": 0, "dropped": 0, "reconnects": 0}
        self._proc: subprocess.Popen | None = None
        self._opened_at: float = 0.0
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
        tables = _parse_tables(env.get("GEODB_DERIVED_MIRROR_TABLES", ""))
        if not tables:
            logger.warning(
                "geodb_derived.misconfigured: GEODB_DERIVED_MIRROR_TABLES names no known table; mirror disabled"
            )
            return None
        refresh = _env_number(env, "GEODB_DERIVED_SESSION_REFRESH_S", DEFAULT_SESSION_REFRESH_S, float)
        retries = max(0, _env_number(env, "GEODB_DERIVED_RETRIES", DEFAULT_RETRIES, int))
        rate = _env_number(env, "GEODB_DERIVED_MAX_ROWS_PER_S", 0.0, float)
        batch_rows = _env_number(env, "GEODB_DERIVED_BATCH_ROWS", BATCH_MAX, int)
        table_rates = {
            t: _env_number(env, f"GEODB_DERIVED_MAX_ROWS_PER_S_{t.upper()}", rate, float) for t in TABLES
        }
        table_batch_rows = {
            t: _env_number(env, f"GEODB_DERIVED_BATCH_ROWS_{t.upper()}", batch_rows, int) for t in TABLES
        }
        return cls(
            [env.get("GEODB_CLIENT_BIN") or "geodb-client", "--key", key, "--binding", binding, "derived", "pipe"],
            budget=ms / 1000,
            batch=env.get("GEODB_DERIVED_BATCH", "").strip() in ("1", "true"),
            tables=tables,
            session_refresh=refresh,
            retries=retries,
            max_rows_per_s=rate,
            batch_rows=batch_rows,
            table_rates=table_rates,
            table_batch_rows=table_batch_rows,
        )

    # -- public ---------------------------------------------------------------------------

    def enabled(self, table: str) -> bool:
        """Whether ``table`` is one of the tables this mirror writes."""
        return table in self.tables

    def skipped(self, error: Exception | str, table: str = "") -> None:
        """Counts a row that cannot be mirrored."""
        self.stats["skipped"] += 1
        logger.warning("geodb_derived.row_skipped table=%s error=%s", table, error)

    def put(self, table: str, rows: Iterable[DerivedRow]) -> None:
        """Mirrors ``rows`` of ``table`` (a label for the logs). Never raises.

        ``rows`` is consumed lazily, so a generator over millions of rows is not held in memory.
        """
        try:
            self.stats["puts"] += self._send(table, self._lines(table, rows))
        except Exception:  # noqa: BLE001 - nothing here may fail the writer
            self.stats["errors"] += 1
            logger.exception("geodb_derived.mirror_failed table=%s; Postgres is unaffected", table)

    def _lines(self, table: str, rows: Iterable[DerivedRow]) -> Iterable[str]:
        for r in rows:
            try:
                yield r.put_line()
            except ValueError as e:
                self.skipped(e, table)

    def close(self) -> None:
        self._drop()

    # -- internals ------------------------------------------------------------------------

    def _send(self, table: str, lines: Iterable[str]) -> int:
        start = time.monotonic()
        sent = 0
        size = self.table_batch_rows.get(table, self.batch_rows) if self.batch else CHUNK
        rate = self.table_rates.get(table, self.max_rows_per_s)
        reconnects = 0
        paused = 0.0  # seconds spent waiting for the pace; not counted against the budget
        it = iter(lines)
        while chunk := list(islice(it, size)):
            if sent > 0 and time.monotonic() - start - paused > self.budget:
                dropped = len(chunk) + sum(1 for _ in it)
                self.stats["dropped"] += dropped
                logger.warning(
                    "geodb_derived.budget_spent table=%s dropped=%d; the rest is left for the next run",
                    table,
                    dropped,
                )
                break
            reqs = [batch_request(chunk)] if self.batch and len(chunk) > 1 else chunk
            chunk_start = time.monotonic()
            try:
                reconnects = self._request_chunk(table, reqs, reconnects)
            except _CoolingDown:
                self.stats["errors"] += 1
                self.stats["dropped"] += len(chunk) + sum(1 for _ in it)
                break
            except Exception as e:  # noqa: BLE001
                logger.warning("geodb_derived.mirror_failed table=%s error=%s; Postgres is unaffected", table, e)
                self.stats["errors"] += 1
                self.stats["dropped"] += len(chunk) + sum(1 for _ in it)
                break
            sent += len(chunk)
            if rate:
                wait = len(chunk) / rate - (time.monotonic() - chunk_start)
                if wait > 0:
                    time.sleep(wait)
                    paused += wait
        return sent

    def _request_chunk(self, table: str, reqs: list[str], reconnects: int) -> int:
        """Sends one chunk, re-handshaking and resending it when the session is lost or expired.

        Returns the call's reconnect count. Raises the last error once ``retries`` resends of this
        chunk, or ``MAX_RECONNECTS`` in the call, are spent, or at once for an error that a new
        session cannot cure.
        """
        self._refresh_if_aging(table)
        attempt = 0
        while True:
            try:
                self._request_all(reqs, fresh=attempt > 0)
                return reconnects
            except _CoolingDown:
                raise
            except Exception as e:  # noqa: BLE001
                if attempt >= self.retries or reconnects >= MAX_RECONNECTS or not _retryable(e):
                    raise
                attempt += 1
                reconnects += 1
                self.stats["reconnects"] += 1
                logger.warning(
                    "geodb_derived.reconnect table=%s attempt=%d error=%s; new session, same chunk",
                    table,
                    attempt,
                    e,
                )
                time.sleep(self.retry_delay * 2 ** (attempt - 1))

    def _refresh_if_aging(self, table: str) -> None:
        """Replaces a session older than ``session_refresh`` before its grant can expire."""
        if self._proc is not None and time.monotonic() - self._opened_at > self.session_refresh:
            logger.info("geodb_derived.session_refresh table=%s; the session is older than %.0fs", table, self.session_refresh)
            self._drop()

    def _request_all(self, reqs: list[str], fresh: bool = False) -> None:
        """Sends requests in order over the held child; the first failure drops it.

        ``fresh`` ignores the cooldown: the caller is resending after a failure it means to cure.
        """
        proc = self._ensure(ignore_cooldown=fresh)
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

    def _ensure(self, ignore_cooldown: bool = False) -> subprocess.Popen:
        if self._proc is not None:
            return self._proc
        if not ignore_cooldown and self._failed_at is not None and time.monotonic() - self._failed_at < self.cooldown:
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
        self._opened_at = time.monotonic()
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


def _retryable(error: Exception) -> bool:
    """Whether a new session can cure ``error``: a lost pipe, an unready child, or an expired grant.

    Any other refusal (``refused: DefiniteRefusal ...``) is the serve's answer to the request itself
    and would be the same on a new session.
    """
    text = str(error)
    if text.startswith("refused:"):
        return "CapabilityTokenExpired" in text
    return True


def _rate(value: float) -> float:
    """``value`` when it is a positive finite number, else 0 (unlimited)."""
    return value if math.isfinite(value) and value > 0 else 0.0


def _batch_rows(value: int) -> int:
    return min(max(1, int(value)), BATCH_MAX)


def _env_number(env: Mapping[str, str], name: str, default, kind):
    try:
        return kind(env.get(name, "").strip() or default)
    except ValueError:
        return default
