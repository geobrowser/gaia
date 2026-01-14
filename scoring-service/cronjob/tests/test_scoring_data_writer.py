"""Unit tests for the ScoringDataWriter module."""

from datetime import datetime
from unittest.mock import ANY, MagicMock, patch, call

import pytest

from src.algorithm.models import Entity, Perspective, Space
from src.scoring_data_writer import ScoringDataWriter


class TestScoringDataWriter:
    """Tests for the ScoringDataWriter class."""

    def test_write_all_calls_all_write_methods(self) -> None:
        """Test that write_all calls all three write methods."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")

            # Create test data
            entity = Entity(id="entity-1", created_at=datetime.now())
            entity.normalized_score = 0.5
            entity.perspectives = [
                Perspective(
                    id="entity-1_space-1",
                    entity_id="entity-1",
                    space_id="space-1",
                    created_at=datetime.now(),
                )
            ]
            entity.perspectives[0].normalized_score = 0.75

            space = Space(id="space-1", created_at=datetime.now())
            space.space_score = 0.8

            # Patch private methods to verify they're called
            with patch.object(writer, "_write_global_scores") as mock_global, \
                 patch.object(writer, "_write_local_scores") as mock_local, \
                 patch.object(writer, "_write_space_scores") as mock_space:

                writer.write_all([entity], [space])

                mock_global.assert_called_once_with(mock_conn, [entity], ANY)
                mock_local.assert_called_once_with(mock_conn, [entity], ANY)
                mock_space.assert_called_once_with(mock_conn, [space], ANY)

    def test_write_global_scores_inserts_entity_scores(self) -> None:
        """Test that _write_global_scores inserts entity normalized scores."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")

            entity1 = Entity(id="entity-1", created_at=datetime.now())
            entity1.normalized_score = 0.5

            entity2 = Entity(id="entity-2", created_at=datetime.now())
            entity2.normalized_score = 0.75

            now = datetime.now()
            writer._write_global_scores(mock_conn, [entity1, entity2], now)

            # Verify executemany was called
            mock_cursor.executemany.assert_called_once()

            # Verify the SQL and data
            call_args = mock_cursor.executemany.call_args
            sql = call_args[0][0]
            data = call_args[0][1]

            assert "INSERT INTO global_scores" in sql
            assert "ON CONFLICT (entity_id) DO UPDATE" in sql
            assert len(data) == 2
            assert data[0][0] == "entity-1"
            assert data[0][1] == 0.5
            assert data[1][0] == "entity-2"
            assert data[1][1] == 0.75

    def test_write_local_scores_inserts_perspective_scores(self) -> None:
        """Test that _write_local_scores inserts perspective normalized scores."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")

            perspective1 = Perspective(
                id="entity-1_space-1",
                entity_id="entity-1",
                space_id="space-1",
                created_at=datetime.now(),
            )
            perspective1.normalized_score = 0.6

            perspective2 = Perspective(
                id="entity-1_space-2",
                entity_id="entity-1",
                space_id="space-2",
                created_at=datetime.now(),
            )
            perspective2.normalized_score = 0.4

            entity = Entity(id="entity-1", created_at=datetime.now())
            entity.perspectives = [perspective1, perspective2]

            now = datetime.now()
            writer._write_local_scores(mock_conn, [entity], now)

            # Verify executemany was called
            mock_cursor.executemany.assert_called_once()

            # Verify the SQL and data
            call_args = mock_cursor.executemany.call_args
            sql = call_args[0][0]
            data = call_args[0][1]

            assert "INSERT INTO local_scores" in sql
            assert "ON CONFLICT (entity_id, space_id) DO UPDATE" in sql
            assert len(data) == 2
            assert data[0][0] == "entity-1"
            assert data[0][1] == "space-1"
            assert data[0][2] == 0.6
            assert data[1][0] == "entity-1"
            assert data[1][1] == "space-2"
            assert data[1][2] == 0.4

    def test_write_space_scores_inserts_space_scores(self) -> None:
        """Test that _write_space_scores inserts space scores."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")

            space1 = Space(id="space-1", created_at=datetime.now())
            space1.space_score = 0.8

            space2 = Space(id="space-2", created_at=datetime.now())
            space2.space_score = 0.64

            now = datetime.now()
            writer._write_space_scores(mock_conn, [space1, space2], now)

            # Verify executemany was called
            mock_cursor.executemany.assert_called_once()

            # Verify the SQL and data
            call_args = mock_cursor.executemany.call_args
            sql = call_args[0][0]
            data = call_args[0][1]

            assert "INSERT INTO space_scores" in sql
            assert "ON CONFLICT (space_id) DO UPDATE" in sql
            assert len(data) == 2
            assert data[0][0] == "space-1"
            assert data[0][1] == 0.8
            assert data[1][0] == "space-2"
            assert data[1][1] == 0.64

    def test_write_global_scores_handles_empty_list(self) -> None:
        """Test that _write_global_scores handles empty entity list."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")
            now = datetime.now()
            writer._write_global_scores(mock_conn, [], now)

            # Should not call executemany for empty list
            mock_cursor.executemany.assert_not_called()

    def test_write_local_scores_handles_empty_perspectives(self) -> None:
        """Test that _write_local_scores handles entities with no perspectives."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")

            entity = Entity(id="entity-1", created_at=datetime.now())
            entity.perspectives = []

            now = datetime.now()
            writer._write_local_scores(mock_conn, [entity], now)

            # Should not call executemany for empty perspectives
            mock_cursor.executemany.assert_not_called()

    def test_write_space_scores_handles_empty_list(self) -> None:
        """Test that _write_space_scores handles empty space list."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")
            now = datetime.now()
            writer._write_space_scores(mock_conn, [], now)

            # Should not call executemany for empty list
            mock_cursor.executemany.assert_not_called()

    def test_write_all_with_multiple_entities_and_spaces(self) -> None:
        """Test write_all with multiple entities having multiple perspectives."""
        with patch("src.scoring_data_writer.scoring_data_writer.psycopg.connect") as mock_connect:
            mock_conn = MagicMock()
            mock_cursor = MagicMock()
            mock_connect.return_value.__enter__.return_value = mock_conn
            mock_conn.cursor.return_value.__enter__.return_value = mock_cursor

            writer = ScoringDataWriter("postgresql://test:test@localhost/test")

            # Entity 1 with 2 perspectives
            perspective1 = Perspective(
                id="entity-1_space-1",
                entity_id="entity-1",
                space_id="space-1",
                created_at=datetime.now(),
            )
            perspective1.normalized_score = 0.5

            perspective2 = Perspective(
                id="entity-1_space-2",
                entity_id="entity-1",
                space_id="space-2",
                created_at=datetime.now(),
            )
            perspective2.normalized_score = 0.6

            entity1 = Entity(id="entity-1", created_at=datetime.now())
            entity1.normalized_score = 0.55
            entity1.perspectives = [perspective1, perspective2]

            # Entity 2 with 1 perspective
            perspective3 = Perspective(
                id="entity-2_space-1",
                entity_id="entity-2",
                space_id="space-1",
                created_at=datetime.now(),
            )
            perspective3.normalized_score = 0.8

            entity2 = Entity(id="entity-2", created_at=datetime.now())
            entity2.normalized_score = 0.8
            entity2.perspectives = [perspective3]

            # Spaces
            space1 = Space(id="space-1", created_at=datetime.now())
            space1.space_score = 0.8

            space2 = Space(id="space-2", created_at=datetime.now())
            space2.space_score = 0.64

            writer.write_all([entity1, entity2], [space1, space2])

            # Verify executemany was called 3 times (global, local, space)
            assert mock_cursor.executemany.call_count == 3

            # Verify data counts in each call
            calls = mock_cursor.executemany.call_args_list

            # Global scores: 2 entities
            global_data = calls[0][0][1]
            assert len(global_data) == 2

            # Local scores: 3 perspectives
            local_data = calls[1][0][1]
            assert len(local_data) == 3

            # Space scores: 2 spaces
            space_data = calls[2][0][1]
            assert len(space_data) == 2
