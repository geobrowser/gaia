"""ScoringDataWriter module for persisting calculated scores to PostgreSQL."""

from datetime import datetime

import psycopg

from src.algorithm.models import Entity, Space


class ScoringDataWriter:
    """Persists calculated scores to PostgreSQL using batch upserts."""

    def __init__(self, connection_string: str):
        """Initialize the ScoringDataWriter with a database connection string.

        Args:
            connection_string: PostgreSQL connection string (e.g., "postgresql://user:pass@host/db")
        """
        self._connection_string = connection_string

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
        with psycopg.connect(self._connection_string) as conn:
            with conn.transaction():
                self._write_global_scores(conn, entities, now)
                self._write_local_scores(conn, entities, now)
                self._write_space_scores(conn, spaces, now)

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
