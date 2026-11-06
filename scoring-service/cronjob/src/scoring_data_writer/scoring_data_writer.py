"""ScoringDataWriter module for persisting calculated scores to PostgreSQL."""

import logging
import time
from datetime import datetime

import psycopg

from src.algorithm.models import Entity, Space
from src.geodb_derived import DerivedMirror, global_rows, local_rows, space_rows

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
        with psycopg.connect(self._connection_string) as conn:
            with conn.transaction():
                t0 = time.monotonic()
                self._write_global_scores(conn, entities, now)
                logger.info("Wrote global scores (%d entities) in %.1fs", len(entities), time.monotonic() - t0)

                t0 = time.monotonic()
                self._write_local_scores(conn, entities, now)
                logger.info("Wrote local scores (%d perspectives) in %.1fs", total_perspectives, time.monotonic() - t0)

                t0 = time.monotonic()
                self._write_space_scores(conn, spaces, now)
                logger.info("Wrote space scores (%d spaces) in %.1fs", len(spaces), time.monotonic() - t0)

        # Only after the transaction has committed. Never raises.
        self._mirror_to_geodb(entities, spaces)

    def _mirror_to_geodb(self, entities: list[Entity], spaces: list[Space]) -> None:
        """Copy the rows just written into geo-db's derived class (collections 2, 3, 4).

        Best effort: a failure is logged and counted and never propagates, so Postgres (the source
        of truth) and the Kafka emit that follows are unaffected. The cron rewrites every row each
        run, so a row dropped here is repaired by the next run.
        """
        mirror = self._derived
        if mirror is None:
            return
        try:
            t0 = time.monotonic()
            before = dict(mirror.stats)
            skip = lambda e: mirror.skipped(e)  # noqa: E731
            # Per-table selection (GEODB_DERIVED_MIRROR_TABLES); all three by default.
            if mirror.enabled("global_scores"):
                mirror.put("global_scores", global_rows(entities, skip))
            if mirror.enabled("local_scores"):
                mirror.put("local_scores", local_rows(entities, skip))
            if mirror.enabled("space_scores"):
                mirror.put("space_scores", space_rows(spaces, skip))
            delta = {k: mirror.stats[k] - before[k] for k in before}
            logger.info(
                "geodb_derived mirror finished in %.1fs: puts=%d errors=%d skipped=%d dropped=%d",
                time.monotonic() - t0,
                delta["puts"],
                delta["errors"],
                delta["skipped"],
                delta["dropped"],
            )
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
