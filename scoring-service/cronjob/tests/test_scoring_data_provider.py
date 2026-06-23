"""Unit tests for the ScoringDataProvider module."""

import uuid
from datetime import datetime
from unittest.mock import MagicMock, patch

import pytest

from src.algorithm.models import (
    DISCONNECTED_SPACE_DEPTH,
    SPACE_SCORE_DECAY_BASE,
    Space,
    VoteType,
)
from src.constants import ROOT_SPACE_ID
from src.scoring_data_provider import ScoringDataProvider, ScoringData


class TestScoringDataProvider:
    """Tests for the ScoringDataProvider class."""

    def test_fetch_all_returns_scoring_data(self) -> None:
        """Test that fetch_all returns a ScoringData object with all data."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            # Setup mock connection and cursor
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            # Setup mock data for each query
            mock_cursor.fetchall.side_effect = [
                # spaces query
                [("space-1",), (ROOT_SPACE_ID,), ("space-2",)],
                # topology distances query (empty = fallback to flat hierarchy)
                [],
                # members query
                [("0xuser1", "space-1"), ("0xuser2", "space-2"), ("0xuser3", "space-1")],
                # editors query
                [("0xuser1", "space-2")],
                # votes query
                [
                    # vote_type encoding: 0=up, 1=down, 2=remove
                    ("0xuser1", "entity-1", "space-1", 0, datetime(2024, 1, 1)),
                    ("0xuser2", "entity-1", "space-1", 1, datetime(2024, 1, 2)),
                ],
                # perspectives query (values)
                [("entity-1", "space-1"), ("entity-1", "space-2")],
                # entities query
                [("entity-1", "1762992995")],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            result = provider.fetch_all()

            assert isinstance(result, ScoringData)
            assert len(result.spaces) == 3
            assert len(result.users) == 3
            assert len(result.votes) == 2
            assert len(result.entities) == 1

    def test_fetch_spaces_with_subspace_relationships(self) -> None:
        """Test that spaces are fetched with correct parent/child relationships."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.side_effect = [
                # spaces query
                [("space-parent",), ("space-child",), (ROOT_SPACE_ID,)],
                # topology distances query (empty = fallback to flat hierarchy)
                [],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            spaces = provider._fetch_spaces(mock_conn)

            assert len(spaces) == 3

            root_space = next(s for s in spaces if s.id == ROOT_SPACE_ID)
            assert root_space.parent_space_id is None
            assert len(root_space.subspace_ids) == 2
            assert "space-parent" in root_space.subspace_ids
            assert "space-child" in root_space.subspace_ids

            child_space = next(s for s in spaces if s.id == "space-child")
            assert child_space.parent_space_id == ROOT_SPACE_ID

    def test_fetch_users_aggregates_memberships(self) -> None:
        """Test that users are aggregated with both member and editor spaces."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.side_effect = [
                # members query
                [("0xUser1", "space-1"), ("0xUser1", "space-2")],
                # editors query
                [("0xUser1", "space-3")],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            users = provider._fetch_users(mock_conn)

            assert len(users) == 1
            user = users[0]

            assert user.id == "0xuser1"  # Lowercased
            assert "space-1" in user.member_spaces
            assert "space-2" in user.member_spaces
            assert "space-3" in user.editor_spaces

    def test_fetch_votes_maps_vote_types(self) -> None:
        """Test that vote types are correctly mapped."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.return_value = [
                ("0xuser1", "entity-1", "space-1", 0, datetime(2024, 1, 1)),  # Up
                ("0xuser2", "entity-1", "space-1", 1, datetime(2024, 1, 2)),  # Down
                ("0xuser3", "entity-1", "space-1", 2, datetime(2024, 1, 3)),  # Remove (ignored)
                ("0xuser4", "entity-1", "space-1", 99, datetime(2024, 1, 4)),  # Invalid (ignored)
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            votes = provider._fetch_votes(mock_conn)

            # Should only have 2 valid votes (remove/invalid are skipped)
            assert len(votes) == 2

            upvote = next(v for v in votes if v.user_id == "0xuser1")
            downvote = next(v for v in votes if v.user_id == "0xuser2")

            assert upvote.vote_type == VoteType.UPVOTE
            assert downvote.vote_type == VoteType.DOWNVOTE

            # Ensure query is filtering to entity-only votes.
            executed_sql = mock_cursor.execute.call_args[0][0]
            assert "FROM user_votes" in executed_sql
            assert "WHERE object_type = 0" in executed_sql

    def test_fetch_perspectives_creates_unique_pairs(self) -> None:
        """Test that perspectives are created from unique entity_id, space_id pairs."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.return_value = [
                ("entity-1", "space-1"),
                ("entity-1", "space-2"),
                ("entity-2", "space-1"),
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            perspectives = provider._fetch_perspectives(mock_conn)

            assert len(perspectives) == 3

            # Check perspective IDs are generated correctly
            perspective_ids = {p.id for p in perspectives}
            assert "entity-1_space-1" in perspective_ids
            assert "entity-1_space-2" in perspective_ids
            assert "entity-2_space-1" in perspective_ids

    def test_build_entities_with_perspectives(self) -> None:
        """Test that perspectives are correctly attached to entities."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            # Setup mock data
            mock_cursor.fetchall.side_effect = [
                # spaces query
                [("space-1",)],
                # topology distances query (empty = fallback to flat hierarchy)
                [],
                # members query
                [],
                # editors query
                [],
                # votes query
                [],
                # perspectives query
                [("entity-1", "space-1"), ("entity-1", "space-2")],
                # entities query
                [("entity-1", "1765466943")],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            result = provider.fetch_all()

            entity = result.entities[0]
            assert len(entity.perspectives) == 2
            assert len(entity.perspective_ids) == 2

            perspective_space_ids = {p.space_id for p in entity.perspectives}
            assert "space-1" in perspective_space_ids
            assert "space-2" in perspective_space_ids

    def test_fetch_entities_parses_unix_timestamp(self) -> None:
        """Test that entity created_at is correctly parsed from Unix timestamp."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.return_value = [
                ("entity-1", "1765466943"),
                ("entity-2", "1765466943"),
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            entities = provider._fetch_entities(mock_conn)

            assert len(entities) == 2
            # Unix timestamp 1765466943 = December 11, 2025
            assert entities[0].created_at.year == 2025
            assert entities[0].created_at.month == 12
            assert entities[0].created_at.day == 11

    def test_empty_database_returns_empty_lists(self) -> None:
        """Test that empty database returns empty lists."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            # All queries return empty
            mock_cursor.fetchall.return_value = []

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            result = provider.fetch_all()

            assert result.entities == []
            assert result.votes == []
            assert result.users == []
            assert result.spaces == []

    def test_fetch_spaces_uses_topology_distances_when_available(self) -> None:
        """Test that spaces use pre-computed topology distances when available."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.side_effect = [
                # spaces query
                [(ROOT_SPACE_ID,), ("space-a",), ("space-b",), ("space-c",)],
                # topology distances query
                [
                    (ROOT_SPACE_ID, 0),
                    ("space-a", 1),
                    ("space-b", 2),
                ],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            spaces = provider._fetch_spaces(mock_conn)

            assert len(spaces) == 4

            root = next(s for s in spaces if s.id == ROOT_SPACE_ID)
            assert root.distance_to_root == 0
            assert root.parent_space_id is None

            space_a = next(s for s in spaces if s.id == "space-a")
            assert space_a.distance_to_root == 1

            space_b = next(s for s in spaces if s.id == "space-b")
            assert space_b.distance_to_root == 2

            # space-c not in topology → gets DISCONNECTED_SPACE_DEPTH
            space_c = next(s for s in spaces if s.id == "space-c")
            assert space_c.distance_to_root == 11

    def test_fetch_spaces_falls_back_to_flat_hierarchy(self) -> None:
        """Test that spaces fall back to flat hierarchy when no topology distances."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.side_effect = [
                # spaces query
                [(ROOT_SPACE_ID,), ("space-1",)],
                # topology distances query — empty (indexer hasn't run)
                [],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            spaces = provider._fetch_spaces(mock_conn)

            assert len(spaces) == 2

            root = next(s for s in spaces if s.id == ROOT_SPACE_ID)
            assert root.parent_space_id is None
            assert "space-1" in root.subspace_ids
            # Flat fallback sets distances explicitly: root=0, every other space=1.
            assert root.distance_to_root == 0

            child = next(s for s in spaces if s.id == "space-1")
            assert child.parent_space_id == ROOT_SPACE_ID
            assert child.distance_to_root == 1

    def test_flat_fallback_identifies_root_with_uuid_ids(self) -> None:
        """Flat fallback must detect the root even when spaces.id rows are uuid.UUID.

        psycopg returns UUID columns as uuid.UUID objects, not strings. Comparing the
        raw uuid.UUID against the string ROOT_SPACE_ID is always False, which would
        misclassify the root as a child (distance 1, self-parent). Root detection must
        use the string form.
        """
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            child_id = uuid.uuid4()
            mock_cursor.fetchall.side_effect = [
                # spaces query — ids come back as uuid.UUID objects (production behavior)
                [(uuid.UUID(ROOT_SPACE_ID),), (child_id,)],
                # topology distances query — empty (indexer hasn't run)
                [],
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            spaces = provider._fetch_spaces(mock_conn)

            root = next(s for s in spaces if s.id == ROOT_SPACE_ID)
            assert root.parent_space_id is None
            assert root.distance_to_root == 0
            assert str(child_id) in root.subspace_ids
            assert ROOT_SPACE_ID not in root.subspace_ids

            child = next(s for s in spaces if s.id == str(child_id))
            assert child.parent_space_id == ROOT_SPACE_ID
            assert child.distance_to_root == 1

    def test_fetch_scoring_topology_distances(self) -> None:
        """Test that topology distances are fetched correctly."""
        with patch("src.scoring_data_provider.scoring_data_provider.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            mock_cursor.fetchall.return_value = [
                (ROOT_SPACE_ID, 0),
                ("space-a", 1),
                ("space-b", 3),
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            distances = provider._fetch_scoring_topology_distances(mock_conn)

            assert distances[ROOT_SPACE_ID] == 0
            assert distances["space-a"] == 1
            assert distances["space-b"] == 3


class TestSpaceScoreCalculation:
    """Tests for space score calculation with topology distances."""

    def test_root_space_has_highest_score(self) -> None:
        """Test that root space (distance=0) scores higher than any child space."""
        now = datetime.now()
        root = Space(id=ROOT_SPACE_ID, created_at=now, distance_to_root=0)
        child_1 = Space(id="child-1", created_at=now, distance_to_root=1)
        child_3 = Space(id="child-3", created_at=now, distance_to_root=3)
        spaces = [root, child_1, child_3]

        for space in spaces:
            space.calculate_space_score(spaces, {}, {})

        assert root.space_score == 1.0
        assert root.space_score > child_1.space_score
        assert child_1.space_score > child_3.space_score

    def test_unset_distance_defaults_to_disconnected(self) -> None:
        """A space with no distance (no topology entry) is scored as disconnected."""
        now = datetime.now()
        space = Space(id="orphan", created_at=now, distance_to_root=None)

        space.calculate_space_score([space], {}, {})

        assert space.distance_to_root == DISCONNECTED_SPACE_DEPTH
        assert space.space_score == SPACE_SCORE_DECAY_BASE**DISCONNECTED_SPACE_DEPTH
