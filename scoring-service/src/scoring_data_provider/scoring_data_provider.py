"""ScoringDataProvider module for fetching and aggregating scoring data from PostgreSQL."""

from dataclasses import dataclass
from datetime import datetime

import psycopg

from src.algorithm.models import Entity, Perspective, Space, User, Vote, VoteType
from src.constants import ROOT_SPACE_ID

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
            spaces = self._fetch_spaces(conn)
            users = self._fetch_users(conn)
            votes = self._fetch_votes(conn)
            perspectives = self._fetch_perspectives(conn)
            entities = self._fetch_entities(conn)
            entities = self._build_entities_with_perspectives(entities, perspectives)

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
                SELECT id, created_at
                FROM entities
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

    def _fetch_spaces(self, conn: psycopg.Connection) -> list[Space]:
        """Fetch all spaces and their subspace relationships from the database.

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

        # Build Space objects - all spaces are children of the root space
        spaces = []

        non_root_spaces = {str(space_id) for (space_id,) in space_rows if space_id != ROOT_SPACE_ID}

        for (space_id,) in space_rows:
            space_id_str = str(space_id)
            sub_space_ids = non_root_spaces if space_id == ROOT_SPACE_ID else set()
            parent_space_id = None if space_id == ROOT_SPACE_ID else ROOT_SPACE_ID
            spaces.append(
                Space(
                    id=space_id_str,
                    # FIXME: add space creation time
                    created_at=datetime.now(),
                    parent_space_id=parent_space_id,
                    subspace_ids=sub_space_ids,
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
                SELECT address, space_id
                FROM members
                """
            )
            member_rows = cur.fetchall()

        # Fetch editors
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT address, space_id
                FROM editors
                """
            )
            editor_rows = cur.fetchall()

        # Aggregate by user address
        user_members: dict[str, set[str]] = {}
        user_editors: dict[str, set[str]] = {}

        for address, space_id in member_rows:
            address_str = str(address).lower()
            space_id_str = str(space_id)

            if address_str not in user_members:
                user_members[address_str] = set()
            user_members[address_str].add(space_id_str)

        for address, space_id in editor_rows:
            address_str = str(address).lower()
            space_id_str = str(space_id)

            if address_str not in user_editors:
                user_editors[address_str] = set()
            user_editors[address_str].add(space_id_str)

        # Build User objects for all unique addresses
        all_addresses = set(user_members.keys()) | set(user_editors.keys())
        users = []

        for address in all_addresses:
            users.append(
                User(
                    id=address,
                    member_spaces=user_members.get(address, set()),
                    editor_spaces=user_editors.get(address, set()),
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
                SELECT user_id, entity_id, space_id, vote_type, voted_at
                FROM user_votes
                """
            )
            rows = cur.fetchall()

        votes = []
        for row in rows:
            user_id, entity_id, space_id, vote_type, voted_at = row

            # Map vote_type: 1 = upvote, -1 = downvote
            if vote_type == 1:
                vote_type_enum = VoteType.UPVOTE
            elif vote_type == -1:
                vote_type_enum = VoteType.DOWNVOTE
            else:
                # Skip invalid vote types
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
