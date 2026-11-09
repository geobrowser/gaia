"""ScoringDataWriter module for persisting calculated scores to PostgreSQL."""

import logging
import time
from datetime import datetime

import psycopg

from src.algorithm.models import Entity, Space
from src.geodb_derived import DerivedMirror, global_rows, key_bytes, local_rows, norm_id as _norm, space_rows

MIRROR_TABLES = ("global_scores", "local_scores", "space_scores")
# Tables too large to rewrite every night; the others are always sent whole.
DIFFED_TABLES = ("global_scores", "local_scores")
# How often a table's progress is written while it is being sent, so a killed run loses at most this.
PROGRESS_EVERY_S = 15.0

logger = logging.getLogger(__name__)


def _changed_up_to(table: str, keys: set | None, after_hex: str) -> bool:
    """Whether a changed key of ``table`` sorts at or before ``after_hex`` (``keys`` None: unknown, so yes)."""
    if keys is None:
        return True
    after = bytes.fromhex(after_hex)
    if table == "global_scores":
        return any(key_bytes(k) <= after for k in keys)
    return any(key_bytes(e) + key_bytes(sp) <= after for e, sp in keys)


class ScoringDataWriter:
    """Persists calculated scores to PostgreSQL using batch upserts."""

    def __init__(self, connection_string: str, derived: DerivedMirror | None = None):
        """Initialize the ScoringDataWriter with a database connection string.

        Args:
            connection_string: PostgreSQL connection string (e.g., "postgresql://user:pass@host/db")
            derived: Optional best-effort mirror into geo-db's derived class. ``None`` (the default,
                and what ``GEODB_DERIVED_MIRROR`` unset gives) means no geo-db calls at all.
        """
        self._connection_string = connection_string
        self._derived = derived

    def write_all(self, entities: list[Entity], spaces: list[Space]) -> None:
        """Write all score types in a single atomic transaction.

        All writes are performed atomically - if any write fails, all changes are rolled back.

        Args:
            entities: List of Entity objects with calculated normalized_score and perspectives.
            spaces: List of Space objects with calculated space_score.

        Raises:
            psycopg.Error: If any database operation fails (all changes are rolled back).
        """
        now = datetime.now()
        total_perspectives = sum(len(e.perspectives) for e in entities)
        mirror = self._derived
        tables = [t for t in MIRROR_TABLES if mirror is not None and mirror.enabled(t)]
        changed: dict[str, set] = {}
        with psycopg.connect(self._connection_string) as conn:
            state = self._mirror_state(conn, tables)
            with conn.transaction():
                for table in DIFFED_TABLES:
                    synced, after = state.get(table, (False, None))
                    if synced or after is not None:
                        t0 = time.monotonic()
                        try:
                            with conn.transaction():  # a savepoint: a failed diff must not abort the write
                                changed[table] = self._changed_keys(conn, table, entities)
                        except Exception:  # noqa: BLE001 - fall back to a full rewrite of this table
                            logger.exception("geodb_derived.diff_failed table=%s; sending every row", table)
                            state[table] = (False, None)
                            continue
                        logger.info(
                            "Diffed %s against Postgres in %.1fs: %d changed", table, time.monotonic() - t0, len(changed[table])
                        )
                t0 = time.monotonic()
                self._write_global_scores(conn, entities, now)
                logger.info("Wrote global scores (%d entities) in %.1fs", len(entities), time.monotonic() - t0)

                t0 = time.monotonic()
                self._write_local_scores(conn, entities, now)
                logger.info("Wrote local scores (%d perspectives) in %.1fs", total_perspectives, time.monotonic() - t0)

                t0 = time.monotonic()
                self._write_space_scores(conn, spaces, now)
                logger.info("Wrote space scores (%d spaces) in %.1fs", len(spaces), time.monotonic() - t0)

                # Committed together with the scores: until the mirror below finishes cleanly the
                # tables count as unsynced, so a crash in between leads to a full rewrite next run.
                # A crash before the mirror records progress leaves no resume point either (the diff
                # above is lost with it), so the next run starts the table over.
                self._set_state(conn, [(t, False, None) for t in tables])

        # Only after the transaction has committed. Never raises.
        resume = {t: state.get(t, (False, None))[1] for t in tables}
        self._mirror_to_geodb(entities, spaces, changed, tables, resume)

    @staticmethod
    def _mirror_state(conn: psycopg.Connection, tables: list[str]) -> dict[str, tuple[bool, str | None]]:
        """``{table: (synced, resume_after)}`` from ``score_mirror_state`` for ``tables``.

        ``synced``: the geo-db copy matched Postgres after the previous run, so only changed rows go.
        ``resume_after``: hex key of the last row of an unfinished run, up to which the copy matched.
        Without the column (migration 0121 not applied) the state is read without it; on any other
        error the result is empty (the table is missing before its migration, say): every row is sent.
        """
        if not tables:
            return {}
        for sql, has_resume in (
            ("SELECT table_name, synced, resume_after FROM score_mirror_state", True),
            ("SELECT table_name, synced FROM score_mirror_state", False),
        ):
            try:
                with conn.cursor() as cur:
                    cur.execute(sql)
                    return {r[0]: (bool(r[1]), r[2] if has_resume else None) for r in cur.fetchall() if r[0] in tables}
            except Exception:  # noqa: BLE001
                conn.rollback()
                logger.warning("geodb_derived.state_unreadable; trying a narrower read", exc_info=True)
        logger.warning("geodb_derived.state_unreadable; sending every row")
        return {}

    @staticmethod
    def _set_state(conn: psycopg.Connection, entries: list[tuple[str, bool, str | None]]) -> None:
        """Records ``(table, synced, resume_after)``. Never raises and never aborts the caller's transaction.

        Falls back to ``synced`` alone when the ``resume_after`` column does not exist yet.
        """
        if not entries:
            return
        for sql, rows in (
            (
                """
                INSERT INTO score_mirror_state (table_name, synced, resume_after, updated_at)
                VALUES (%s, %s, %s, now())
                ON CONFLICT (table_name) DO UPDATE SET synced = EXCLUDED.synced,
                    resume_after = EXCLUDED.resume_after, updated_at = EXCLUDED.updated_at
                """,
                entries,
            ),
            (
                """
                INSERT INTO score_mirror_state (table_name, synced, updated_at)
                VALUES (%s, %s, now())
                ON CONFLICT (table_name) DO UPDATE SET synced = EXCLUDED.synced, updated_at = EXCLUDED.updated_at
                """,
                [(t, s) for t, s, _ in entries],
            ),
        ):
            try:
                with conn.transaction():  # a savepoint when nested
                    with conn.cursor() as cur:
                        cur.executemany(sql, rows)
                return
            except Exception:  # noqa: BLE001
                logger.warning("geodb_derived.state_unwritable; trying a narrower write", exc_info=True)
        logger.warning("geodb_derived.state_unwritable; the next run sends every row")

    def _changed_keys(self, conn: psycopg.Connection, table: str, entities: list[Entity]) -> set:
        """Keys of the rows of ``table`` that the coming upsert will add or change.

        Stages the new scores in a temp table and anti-joins the stored ones, so nothing big is
        held here but the keys that differ. Must run before the upsert. Scores compare as numeric,
        the column's own type, so a score the column rounds the same way is unchanged.
        """
        with conn.cursor() as cur:
            if table == "global_scores":
                cur.execute(
                    "CREATE TEMP TABLE mirror_new_global (entity_id uuid, score double precision) ON COMMIT DROP"
                )
                with cur.copy("COPY mirror_new_global (entity_id, score) FROM STDIN") as cp:
                    for e in entities:
                        cp.write_row((e.id, e.normalized_score))
                cur.execute(
                    """
                    SELECT n.entity_id FROM mirror_new_global n
                    LEFT JOIN global_scores g USING (entity_id)
                    WHERE g.entity_id IS NULL OR g.score IS DISTINCT FROM n.score::numeric
                    """
                )
                return {_norm(r[0]) for r in cur}
            cur.execute(
                "CREATE TEMP TABLE mirror_new_local (entity_id uuid, space_id uuid, score double precision) ON COMMIT DROP"
            )
            with cur.copy("COPY mirror_new_local (entity_id, space_id, score) FROM STDIN") as cp:
                for e in entities:
                    for p in e.perspectives:
                        cp.write_row((p.entity_id, p.space_id, p.normalized_score))
            cur.execute(
                """
                SELECT n.entity_id, n.space_id FROM mirror_new_local n
                LEFT JOIN local_scores l ON l.entity_id = n.entity_id AND l.space_id = n.space_id
                WHERE l.entity_id IS NULL OR l.score IS DISTINCT FROM n.score::numeric
                """
            )
            return {(_norm(r[0]), _norm(r[1])) for r in cur}

    def _mirror_to_geodb(
        self,
        entities: list[Entity],
        spaces: list[Space],
        changed: dict[str, set] | None = None,
        tables: list[str] | None = None,
        resume: dict[str, str | None] | None = None,
    ) -> None:
        """Copy the rows just written into geo-db's derived class (collections 2, 3, 4).

        Best effort: a failure is logged and counted and never propagates, so Postgres (the source
        of truth) and the Kafka emit that follows are unaffected.

        The big tables go out in key order. A table in ``changed`` sends only the keys listed there,
        plus, when ``resume[table]`` is set, every row whose key sorts past it (the rest of an
        earlier run the budget cut off); a table not in ``changed`` sends every row. The key of the
        last request geo-db answered ok is written to ``score_mirror_state`` as the run goes, so a cut
        or killed run resumes there. A table whose rows all went through is marked synced, which lets
        the next run send only what changed; any drop or error leaves it unsynced at its last good key.
        """
        mirror = self._derived
        if mirror is None:
            return
        changed = changed or {}
        resume = resume or {}
        try:
            t0 = time.monotonic()
            before = dict(mirror.stats)
            skip = lambda e: mirror.skipped(e)  # noqa: E731
            clean: list[str] = []
            last: dict[str, str] = {}  # hex of the last key sent this run, per table

            def progress_for(table: str):
                saved = time.monotonic()

                def progress(key: bytes) -> None:
                    nonlocal saved
                    last[table] = key.hex()
                    if tables and table in tables and time.monotonic() - saved >= PROGRESS_EVERY_S:
                        saved = time.monotonic()
                        self._save_state([(table, False, last[table])])

                return progress

            def run(table: str, rows) -> None:
                if not mirror.enabled(table):
                    return
                mark = dict(mirror.stats)
                mirror.put(table, rows, progress=progress_for(table))
                if mirror.stats["errors"] == mark["errors"] and mirror.stats["dropped"] == mark["dropped"]:
                    clean.append(table)
                after = resume.get(table)
                logger.info(
                    "geodb_derived table=%s mode=%s puts=%d resumed_after=%s last_key=%s",
                    table,
                    ("resume" if after else "changed") if table in changed else "full",
                    mirror.stats["puts"] - mark["puts"],
                    after or "-",
                    last.get(table, "-"),
                )

            # Key order: entities by id, perspectives by entity then Space, so a cursor means something.
            ordered = sorted(entities, key=lambda e: key_bytes(e.id))
            # Per-table selection (GEODB_DERIVED_MIRROR_TABLES); all three by default.
            ch = changed.get("global_scores")
            after = resume.get("global_scores")
            gafter = bytes.fromhex(after) if after else None
            run(
                "global_scores",
                global_rows(
                    [
                        e
                        for e in ordered
                        if ch is None or _norm(e.id) in ch or (gafter is not None and key_bytes(e.id) > gafter)
                    ],
                    skip,
                ),
            )
            ch = changed.get("local_scores")
            after = resume.get("local_scores")
            run("local_scores", local_rows(ordered, skip, only=ch, after=bytes.fromhex(after) if after else None))
            run("space_scores", space_rows(spaces, skip))
            delta = {k: mirror.stats[k] - before[k] for k in before}
            logger.info(
                "geodb_derived mirror finished in %.1fs: puts=%d errors=%d skipped=%d dropped=%d",
                time.monotonic() - t0,
                delta["puts"],
                delta["errors"],
                delta["skipped"],
                delta["dropped"],
            )
            if tables:
                entries: list[tuple[str, bool, str | None]] = []
                for t in tables:
                    if t in clean:
                        entries.append((t, True, None))
                    elif t in last:
                        entries.append((t, False, last[t]))
                    else:
                        # Nothing went through this run. The old resume point still holds when no
                        # changed row sorts at or before it.
                        old = resume.get(t)
                        keep = old if old and not _changed_up_to(t, changed.get(t), old) else None
                        entries.append((t, False, keep))
                self._save_state(entries)
        except Exception:  # noqa: BLE001 - nothing here may fail the writer
            logger.exception("geodb_derived mirror failed; Postgres is unaffected")

    def _save_state(self, entries: list[tuple[str, bool, str | None]]) -> None:
        """Writes state on a short-lived connection of its own; never raises."""
        try:
            with psycopg.connect(self._connection_string, autocommit=True) as conn:
                self._set_state(conn, entries)
        except Exception:  # noqa: BLE001
            logger.warning("geodb_derived.state_unwritable; the next run resumes from an older point", exc_info=True)

    def _write_global_scores(self, conn: psycopg.Connection, entities: list[Entity], now: datetime) -> None:
        """Batch write entity.normalized_score to global_scores table.

        Args:
            conn: Database connection.
            entities: List of Entity objects with calculated normalized_score.
        """
        if not entities:
            return
        data = [(e.id, e.normalized_score, now) for e in entities]

        with conn.cursor() as cur:
            cur.executemany(
                """
                INSERT INTO global_scores (entity_id, score, updated_at)
                VALUES (%s, %s, %s)
                ON CONFLICT (entity_id) DO UPDATE SET
                    score = EXCLUDED.score,
                    updated_at = EXCLUDED.updated_at
                """,
                data,
            )

    def _write_local_scores(self, conn: psycopg.Connection, entities: list[Entity], now: datetime) -> None:
        """Batch write perspective.normalized_score to local_scores table.

        Args:
            conn: Database connection.
            entities: List of Entity objects with perspectives containing normalized_score.
        """
        # Flatten all perspectives from all entities
        data = []

        for entity in entities:
            for perspective in entity.perspectives:
                data.append((
                    perspective.entity_id,
                    perspective.space_id,
                    perspective.normalized_score,
                    now,
                ))

        if not data:
            return

        with conn.cursor() as cur:
            cur.executemany(
                """
                INSERT INTO local_scores (entity_id, space_id, score, updated_at)
                VALUES (%s, %s, %s, %s)
                ON CONFLICT (entity_id, space_id) DO UPDATE SET
                    score = EXCLUDED.score,
                    updated_at = EXCLUDED.updated_at
                """,
                data,
            )

    def _write_space_scores(self, conn: psycopg.Connection, spaces: list[Space], now: datetime) -> None:
        """Batch write space.space_score to space_scores table.

        Args:
            conn: Database connection.
            spaces: List of Space objects with calculated space_score.
        """
        if not spaces:
            return

        data = [(s.id, s.space_score, now) for s in spaces]

        with conn.cursor() as cur:
            cur.executemany(
                """
                INSERT INTO space_scores (space_id, score, updated_at)
                VALUES (%s, %s, %s)
                ON CONFLICT (space_id) DO UPDATE SET
                    score = EXCLUDED.score,
                    updated_at = EXCLUDED.updated_at
                """,
                data,
            )
