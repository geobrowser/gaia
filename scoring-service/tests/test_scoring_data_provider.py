"""Unit tests for the ScoringDataProvider module."""

from datetime import datetime
from unittest.mock import MagicMock, patch

import pytest

from src.algorithm.models import VoteType
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
                # members query
                [("0xuser1", "space-1"), ("0xuser2", "space-2"), ("0xuser3", "space-1")],
                # editors query
                [("0xuser1", "space-2")],
                # votes query
                [
                    ("0xuser1", "entity-1", "space-1", 1, datetime(2024, 1, 1)),
                    ("0xuser2", "entity-1", "space-1", -1, datetime(2024, 1, 2)),
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
                ("0xuser1", "entity-1", "space-1", 1, datetime(2024, 1, 1)),
                ("0xuser2", "entity-1", "space-1", -1, datetime(2024, 1, 2)),
                ("0xuser3", "entity-1", "space-1", 99, datetime(2024, 1, 3)),  # Invalid
            ]

            provider = ScoringDataProvider("postgresql://test:test@localhost/test")
            votes = provider._fetch_votes(mock_conn)

            # Should only have 2 valid votes (invalid vote_type=99 is skipped)
            assert len(votes) == 2

            upvote = next(v for v in votes if v.user_id == "0xuser1")
            downvote = next(v for v in votes if v.user_id == "0xuser2")

            assert upvote.vote_type == VoteType.UPVOTE
            assert downvote.vote_type == VoteType.DOWNVOTE

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
