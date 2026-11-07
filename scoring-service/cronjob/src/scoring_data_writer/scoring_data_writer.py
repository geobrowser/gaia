"""ScoringDataWriter module for persisting calculated scores to PostgreSQL."""

import logging
import time
from datetime import datetime

import psycopg

from src.algorithm.models import Entity, Space
from src.geodb_derived import DerivedMirror, global_rows, local_rows, norm_id as _norm, space_rows

MIRROR_TABLES = ("global_scores", "local_scores", "space_scores")
# Tables too large to rewrite every night; the others are always sent whole.
DIFFED_TABLES = ("global_scores", "local_scores")

logger = logging.getLogger(__name__)


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
            synced = self._synced_tables(conn, tables)
            with conn.transaction():
                for table in DIFFED_TABLES:
                    if table in synced:
                        t0 = time.monotonic()
                        try:
                            with conn.transaction():  # a savepoint: a failed diff must not abort the write
                                changed[table] = self._changed_keys(conn, table, entities)
                        except Exception:  # noqa: BLE001 - fall back to a full rewrite of this table
                            logger.exception("geodb_derived.diff_failed table=%s; sending every row", table)
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
                self._set_synced(conn, tables, False)

        # Only after the transaction has committed. Never raises.
        self._mirror_to_geodb(entities, spaces, changed, tables)

    @staticmethod
    def _synced_tables(conn: psycopg.Connection, tables: list[str]) -> set[str]:
        """Tables whose geo-db copy matched Postgres after the previous run (``score_mirror_state``).

        Empty on any error (the table is missing before its migration, say): every row is sent.
        """
        if not tables:
            return set()
        try:
            with conn.cursor() as cur:
                cur.execute("SELECT table_name FROM score_mirror_state WHERE synced")
                return {r[0] for r in cur.fetchall()} & set(tables)
        except Exception:  # noqa: BLE001
            conn.rollback()
            logger.warning("geodb_derived.state_unreadable; sending every row", exc_info=True)
            return set()

    @staticmethod
    def _set_synced(conn: psycopg.Connection, tables: list[str], value: bool) -> None:
        """Records ``value`` for ``tables``. Never raises and never aborts the caller's transaction."""
        if not tables:
            return
        try:
            with conn.transaction():  # a savepoint when nested
                with conn.cursor() as cur:
                    cur.executemany(
                        """
                        INSERT INTO score_mirror_state (table_name, synced, updated_at)
                        VALUES (%s, %s, now())
                        ON CONFLICT (table_name) DO UPDATE SET synced = EXCLUDED.synced, updated_at = EXCLUDED.updated_at
                        """,
                        [(t, value) for t in tables],
                    )
        except Exception:  # noqa: BLE001
            logger.warning("geodb_derived.state_unwritable; the next run sends every row", exc_info=True)

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
    ) -> None:
        """Copy the rows just written into geo-db's derived class (collections 2, 3, 4).

        Best effort: a failure is logged and counted and never propagates, so Postgres (the source
        of truth) and the Kafka emit that follows are unaffected. A table in ``changed`` sends only
        the keys listed there; the others send every row. A table whose rows all went through is
        marked synced, which lets the next run send only what changed; any drop or error leaves it
        unsynced and the next run rewrites it whole.
        """
        mirror = self._derived
        if mirror is None:
            return
        changed = changed or {}
        try:
            t0 = time.monotonic()
            before = dict(mirror.stats)
            skip = lambda e: mirror.skipped(e)  # noqa: E731
            clean: list[str] = []

            def run(table: str, rows) -> None:
                if not mirror.enabled(table):
                    return
                mark = dict(mirror.stats)
                mirror.put(table, rows)
                if mirror.stats["errors"] == mark["errors"] and mirror.stats["dropped"] == mark["dropped"]:
                    clean.append(table)
                logger.info(
                    "geodb_derived table=%s mode=%s puts=%d",
                    table,
                    "changed" if table in changed else "full",
                    mirror.stats["puts"] - mark["puts"],
                )

            # Per-table selection (GEODB_DERIVED_MIRROR_TABLES); all three by default.
            ch = changed.get("global_scores")
            run(
                "global_scores",
                global_rows([e for e in entities if ch is None or _norm(e.id) in ch], skip),
            )
            ch = changed.get("local_scores")
            run(
                "local_scores",
                local_rows(entities, skip, only=ch),
            )
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
                with psycopg.connect(self._connection_string, autocommit=True) as conn:
                    self._set_synced(conn, [t for t in tables if t in clean], True)
        except Exception:  # noqa: BLE001 - nothing here may fail the writer
            logger.exception("geodb_derived mirror failed; Postgres is unaffected")

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
