"""ScoringDataProvider module for fetching and aggregating scoring data from PostgreSQL."""

import logging
import time
from dataclasses import dataclass
from datetime import datetime

import psycopg

from src.algorithm.models import (
    DISCONNECTED_SPACE_DEPTH,
    Entity,
    Perspective,
    Space,
    User,
    Vote,
    VoteType,
)
from src.constants import ROOT_SPACE_ID

logger = logging.getLogger(__name__)

@dataclass
class ScoringData:
    """Aggregated data for scoring."""

    entities: list[Entity]
    votes: list[Vote]
    users: list[User]
    spaces: list[Space]


class ScoringDataProvider:
    """Fetches and aggregates data from PostgreSQL for the scoring engine."""

    def __init__(self, connection_string: str):
        """Initialize the ScoringDataProvider with a database connection string.

        Args:
            connection_string: PostgreSQL connection string (e.g., "postgresql://user:pass@host/db")
        """
        self._connection_string = connection_string

    def fetch_all(self) -> ScoringData:
        """Fetch all data required for scoring.

        Returns:
            ScoringData containing entities, votes, users, and spaces.
        """
        with psycopg.connect(self._connection_string) as conn:
            t0 = time.monotonic()
            spaces = self._fetch_spaces(conn)
            logger.info("Fetched %d spaces in %.1fs", len(spaces), time.monotonic() - t0)

            t0 = time.monotonic()
            users = self._fetch_users(conn)
            logger.info("Fetched %d users in %.1fs", len(users), time.monotonic() - t0)

            t0 = time.monotonic()
            votes = self._fetch_votes(conn)
            logger.info("Fetched %d votes in %.1fs", len(votes), time.monotonic() - t0)

            t0 = time.monotonic()
            perspectives = self._fetch_perspectives(conn)
            logger.info("Fetched %d perspectives in %.1fs", len(perspectives), time.monotonic() - t0)

            t0 = time.monotonic()
            entities = self._fetch_entities(conn)
            logger.info("Fetched %d entities in %.1fs", len(entities), time.monotonic() - t0)

            t0 = time.monotonic()
            entities = self._build_entities_with_perspectives(entities, perspectives)
            logger.info("Built entities with perspectives in %.1fs", time.monotonic() - t0)

        return ScoringData(
            entities=entities,
            votes=votes,
            users=users,
            spaces=spaces,
        )

    def _fetch_entities(self, conn: psycopg.Connection) -> list[Entity]:
        """Fetch all entities from the database.

        Args:
            conn: Database connection.

        Returns:
            List of Entity objects.
        """
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT e.id, e.created_at
                FROM entities e
                WHERE EXISTS (SELECT 1 FROM "values" v WHERE v.entity_id = e.id)
                """
            )
            rows = cur.fetchall()

        entities = []
        for row in rows:
            entity_id, created_at = row
            # created_at is stored as text in the schema, parse it
            if isinstance(created_at, str):
                # Try Unix timestamp first (numeric string)
                if created_at.isdigit():
                    created_at_dt = datetime.fromtimestamp(int(created_at))
                else:
                    try:
                        created_at_dt = datetime.fromisoformat(created_at.replace("Z", "+00:00"))
                    except ValueError:
                        created_at_dt = datetime.now()
            else:
                created_at_dt = created_at if created_at else datetime.now()

            entities.append(
                Entity(
                    id=str(entity_id),
                    created_at=created_at_dt,
                )
            )

        return entities

    def _fetch_scoring_topology_distances(self, conn: psycopg.Connection) -> dict[str, int]:
        """Fetch pre-computed topology distances from the scoring_topology_distances table.

        Returns:
            Dictionary mapping space_id -> distance from root.
            Empty dict if the table has no data (indexer hasn't run yet).
        """
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT space_id, distance
                FROM scoring_topology_distances
                """
            )
            rows = cur.fetchall()

        return {str(space_id): distance for space_id, distance in rows}

    def _fetch_spaces(self, conn: psycopg.Connection) -> list[Space]:
        """Fetch all spaces and their subspace relationships from the database.

        If the topology indexer has populated scoring_topology_distances, uses those
        pre-computed distances. Otherwise falls back to the flat hierarchy where all
        spaces are direct children of ROOT_SPACE_ID.

        Args:
            conn: Database connection.

        Returns:
            List of Space objects with parent/child relationships populated.
        """
        # Fetch spaces
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT id
                FROM spaces
                """
            )
            space_rows = cur.fetchall()

        # Try to use pre-computed topology distances
        topology_distances = self._fetch_scoring_topology_distances(conn)

        if topology_distances:
            logger.info(
                "Using topology distances (%d entries, root=%s)",
                len(topology_distances),
                next((sid for sid, d in topology_distances.items() if d == 0), "unknown"),
            )
            return self._build_spaces_from_topology(space_rows, topology_distances)

        logger.info("Topology distances empty, using flat hierarchy fallback")
        return self._build_spaces_flat(space_rows)

    def _build_spaces_from_topology(
        self,
        space_rows: list[tuple],
        topology_distances: dict[str, int],
    ) -> list[Space]:
        """Build Space objects using pre-computed topology distances."""
        # Derive root from the distance=0 entry
        root_id = None
        for space_id_str, distance in topology_distances.items():
            if distance == 0:
                root_id = space_id_str
                break

        spaces = []
        for (space_id,) in space_rows:
            space_id_str = str(space_id)
            distance = topology_distances.get(space_id_str, DISCONNECTED_SPACE_DEPTH)

            # Set parent_space_id for compatibility with existing code
            if distance == 0:
                parent_space_id = None
            else:
                parent_space_id = root_id if root_id else ROOT_SPACE_ID

            spaces.append(
                Space(
                    id=space_id_str,
                    created_at=datetime.now(),
                    parent_space_id=parent_space_id,
                    subspace_ids=set(),
                    distance_to_root=distance,
                )
            )

        return spaces

    def _build_spaces_flat(self, space_rows: list[tuple]) -> list[Space]:
        """Build Space objects using flat hierarchy (all children of root).

        Fallback used when the topology indexer has not populated
        scoring_topology_distances yet. Distances are set explicitly here (0 for root,
        1 for every other space as a direct child of root) so the scoring model never
        has to derive them — it only consumes pre-computed distances.
        """
        spaces = []
        non_root_spaces = {str(space_id) for (space_id,) in space_rows if space_id != ROOT_SPACE_ID}

        for (space_id,) in space_rows:
            space_id_str = str(space_id)
            is_root = space_id == ROOT_SPACE_ID
            sub_space_ids = non_root_spaces if is_root else set()
            parent_space_id = None if is_root else ROOT_SPACE_ID
            distance_to_root = 0 if is_root else 1
            spaces.append(
                Space(
                    id=space_id_str,
                    # FIXME: add space creation time
                    created_at=datetime.now(),
                    parent_space_id=parent_space_id,
                    subspace_ids=sub_space_ids,
                    distance_to_root=distance_to_root,
                )
            )

        return spaces

    def _fetch_users(self, conn: psycopg.Connection) -> list[User]:
        """Fetch all users with their memberships and editor roles.

        Args:
            conn: Database connection.

        Returns:
            List of User objects with member_spaces and editor_spaces populated.
        """
        # Fetch members
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT member_space_id, space_id
                FROM members
                """
            )
            member_rows = cur.fetchall()

        # Fetch editors
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT member_space_id, space_id
                FROM editors
                """
            )
            editor_rows = cur.fetchall()

        # Aggregate by user address
        user_members: dict[str, set[str]] = {}
        user_editors: dict[str, set[str]] = {}

        for member_space_id, space_id in member_rows:
            member_space_id_str = str(member_space_id).lower()
            space_id_str = str(space_id)

            if member_space_id_str not in user_members:
                user_members[member_space_id_str] = set()
            user_members[member_space_id_str].add(space_id_str)

        for member_space_id, space_id in editor_rows:
            member_space_id_str = str(member_space_id).lower()
            space_id_str = str(space_id)

            if member_space_id_str not in user_editors:
                user_editors[member_space_id_str] = set()
            user_editors[member_space_id_str].add(space_id_str)

        # Build User objects for all unique member space IDs
        all_member_space_ids = set(user_members.keys()) | set(user_editors.keys())
        users = []

        for member_space_id in all_member_space_ids:
            users.append(
                User(
                    id=member_space_id,
                    member_spaces=user_members.get(member_space_id, set()),
                    editor_spaces=user_editors.get(member_space_id, set()),
                )
            )

        return users

    def _fetch_votes(self, conn: psycopg.Connection) -> list[Vote]:
        """Fetch all votes from the database.

        Args:
            conn: Database connection.

        Returns:
            List of Vote objects.
        """
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT user_id, object_id, space_id, vote_type, voted_at
                FROM user_votes
                WHERE object_type = 0
                """
            )
            rows = cur.fetchall()

        votes = []
        for row in rows:
            user_id, entity_id, space_id, vote_type, voted_at = row

            # Map vote_type (current DB encoding):
            # 0 = upvote, 1 = downvote, 2 = remove (ignore)
            if vote_type == 0:
                vote_type_enum = VoteType.UPVOTE
            elif vote_type == 1:
                vote_type_enum = VoteType.DOWNVOTE
            else:
                # Skip remove/unknown vote types
                continue

            votes.append(
                Vote(
                    user_id=str(user_id).lower(),
                    entity_id=str(entity_id),
                    space_id=str(space_id),
                    vote_type=vote_type_enum,
                    timestamp=voted_at if voted_at else datetime.now(),
                )
            )

        return votes

    def _fetch_perspectives(self, conn: psycopg.Connection) -> list[Perspective]:
        """Fetch all perspectives from the values table.

        Perspectives are derived from unique (entity_id, space_id) pairs in the values table.

        Args:
            conn: Database connection.

        Returns:
            List of Perspective objects.
        """
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT DISTINCT entity_id, space_id
                FROM "values" v
                """
            )
            rows = cur.fetchall()

        perspectives = []
        for row in rows:
            entity_id, space_id = row
            # Generate a perspective ID from entity_id and space_id
            perspective_id = f"{entity_id}_{space_id}"

            perspectives.append(
                Perspective(
                    id=perspective_id,
                    entity_id=str(entity_id),
                    space_id=str(space_id),
                    # FIXME: add perspective creation time
                    created_at=datetime.now(),
                )
            )

        return perspectives

    def _build_entities_with_perspectives(
        self, entities: list[Entity], perspectives: list[Perspective]
    ) -> list[Entity]:
        """Attach perspectives to their respective entities.

        Args:
            entities: List of Entity objects.
            perspectives: List of Perspective objects.

        Returns:
            List of Entity objects with perspectives attached.
        """
        # Build entity_id -> perspectives mapping
        entity_perspectives: dict[str, list[Perspective]] = {}
        for perspective in perspectives:
            if perspective.entity_id not in entity_perspectives:
                entity_perspectives[perspective.entity_id] = []
            entity_perspectives[perspective.entity_id].append(perspective)

        # Attach perspectives to entities
        for entity in entities:
            entity.perspectives = entity_perspectives.get(entity.id, [])
            entity.perspective_ids = [p.id for p in entity.perspectives]

        return entities
