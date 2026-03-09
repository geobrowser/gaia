"""Tests for the main scoring service runtime."""

from datetime import datetime
from unittest.mock import MagicMock, patch

import pytest

from main import OutputMode, ScoringPipeline
from src.algorithm.models import Entity, Space, User, Vote
from src.scoring_data_provider.scoring_data_provider import ScoringData


class TestMain:
    """Tests for the main function."""

    def test_main_orchestrates_scoring_pipeline(self) -> None:
        """Test that main orchestrates provider, engine, and writer correctly."""
        mock_entities = [Entity(id="entity-1", created_at=datetime.now())]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]
        mock_users = [User(id="user-1")]
        mock_votes: list[Vote] = []
        mock_scoring_data = ScoringData(
            entities=mock_entities,
            votes=mock_votes,
            users=mock_users,
            spaces=mock_spaces,
        )

        with (
            patch.dict(
                "os.environ",
                {
                    "DATABASE_URL": "postgresql://test:test@localhost/test",
                    "OUTPUT_MODE": "postgres",
                },
            ),
            patch("main.load_dotenv") as mock_load_dotenv,
            patch("main.ScoringDataProvider") as mock_provider_class,
            patch("main.RankingEngine") as mock_engine_class,
            patch("main.ScoringDataWriter") as mock_writer_class,
        ):
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = mock_scoring_data
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            mock_engine.rank_spaces.return_value = mock_spaces
            mock_engine.rank_entities.return_value = mock_entities
            mock_engine_class.return_value = mock_engine

            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            from main import main

            main()

            mock_load_dotenv.assert_called_once()
            mock_provider_class.assert_called_once_with("postgresql://test:test@localhost/test")
            mock_provider.fetch_all.assert_called_once()
            mock_writer_class.assert_called_once_with("postgresql://test:test@localhost/test")
            mock_writer.write_all.assert_called_once()

    def test_main_exits_when_database_url_missing(self) -> None:
        """Test that main exits with error when DATABASE_URL is not set."""
        with (
            patch.dict("os.environ", {}, clear=True),
            patch("main.load_dotenv"),
            pytest.raises(SystemExit) as exc_info,
        ):
            from main import main

            main()

        assert exc_info.value.code == 1

    def test_main_logs_completion_message(self, caplog: pytest.LogCaptureFixture) -> None:
        """Test that main logs a completion message with counts."""
        mock_entities = [
            Entity(id="entity-1", created_at=datetime.now()),
            Entity(id="entity-2", created_at=datetime.now()),
        ]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]
        mock_scoring_data = ScoringData(
            entities=mock_entities,
            votes=[],
            users=[],
            spaces=mock_spaces,
        )

        with (
            patch.dict(
                "os.environ",
                {
                    "DATABASE_URL": "postgresql://test:test@localhost/test",
                    "OUTPUT_MODE": "postgres",
                },
            ),
            patch("main.load_dotenv"),
            patch("main.ScoringDataProvider") as mock_provider_class,
            patch("main.RankingEngine") as mock_engine_class,
            patch("main.ScoringDataWriter") as mock_writer_class,
            caplog.at_level("INFO"),
        ):
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = mock_scoring_data
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            mock_engine.rank_spaces.return_value = mock_spaces
            mock_engine.rank_entities.return_value = mock_entities
            mock_engine_class.return_value = mock_engine

            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            from main import main

            main()

            assert "Pipeline complete: 2 entities, 1 spaces" in caplog.text


class TestScoringPipeline:
    """Tests for the ScoringPipeline class."""

    def test_run_returns_counts(self) -> None:
        """Test that run returns entity and space counts."""
        mock_entities = [
            Entity(id="entity-1", created_at=datetime.now()),
            Entity(id="entity-2", created_at=datetime.now()),
        ]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]
        mock_scoring_data = ScoringData(
            entities=mock_entities,
            votes=[],
            users=[],
            spaces=mock_spaces,
        )

        with (
            patch("main.ScoringDataProvider") as mock_provider_class,
            patch("main.ScoringDataWriter") as mock_writer_class,
        ):
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = mock_scoring_data
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            mock_engine.rank_spaces.return_value = mock_spaces
            mock_engine.rank_entities.return_value = mock_entities

            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )
            entity_count, space_count = pipeline.run()

            assert entity_count == 2
            assert space_count == 1

    def test_run_modifies_scoring_data_in_place(self) -> None:
        """Test that run stores and modifies scoring_data as instance attribute."""
        mock_entities = [Entity(id="entity-1", created_at=datetime.now())]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]
        ranked_spaces = [Space(id="space-1", created_at=datetime.now(), space_score=0.8)]
        ranked_entities = [Entity(id="entity-1", created_at=datetime.now(), normalized_score=0.5)]
        mock_scoring_data = ScoringData(
            entities=mock_entities,
            votes=[],
            users=[User(id="user-1")],
            spaces=mock_spaces,
        )

        with (
            patch("main.ScoringDataProvider") as mock_provider_class,
            patch("main.ScoringDataWriter") as mock_writer_class,
        ):
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = mock_scoring_data
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            mock_engine.rank_spaces.return_value = ranked_spaces
            mock_engine.rank_entities.return_value = ranked_entities

            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )
            pipeline.run()

            # Verify scoring_data was updated with ranked results
            assert pipeline.scoring_data.spaces == ranked_spaces
            assert pipeline.scoring_data.entities == ranked_entities

    def test_rank_entities_uses_ranked_spaces(self) -> None:
        """Test that _rank_entities uses the already-ranked spaces from scoring_data."""
        mock_entities = [Entity(id="entity-1", created_at=datetime.now())]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]
        ranked_spaces = [Space(id="space-1", created_at=datetime.now(), space_score=0.8)]
        mock_scoring_data = ScoringData(
            entities=mock_entities,
            votes=[],
            users=[User(id="user-1")],
            spaces=mock_spaces,
        )

        with (
            patch("main.ScoringDataProvider") as mock_provider_class,
            patch("main.ScoringDataWriter") as mock_writer_class,
        ):
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = mock_scoring_data
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            mock_engine.rank_spaces.return_value = ranked_spaces
            mock_engine.rank_entities.return_value = mock_entities

            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )
            pipeline.run()

            # Verify rank_entities was called with ranked_spaces (from scoring_data after rank_spaces)
            mock_engine.rank_entities.assert_called_once()
            call_args = mock_engine.rank_entities.call_args
            # 4th positional arg should be ranked_spaces
            assert call_args[0][3] == ranked_spaces

    def test_fetch_data_sets_scoring_data(self) -> None:
        """Test that _fetch_data sets the scoring_data instance attribute."""
        mock_scoring_data = ScoringData(
            entities=[],
            votes=[],
            users=[],
            spaces=[],
        )

        with patch("main.ScoringDataProvider") as mock_provider_class:
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = mock_scoring_data
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )
            assert pipeline.scoring_data is None

            pipeline._fetch_data()

            mock_provider_class.assert_called_once_with("postgresql://test/db")
            mock_provider.fetch_all.assert_called_once()
            assert pipeline.scoring_data == mock_scoring_data

    def test_fetch_data_raises_when_provider_returns_none(self) -> None:
        """Test that _fetch_data raises RuntimeError when provider returns None."""
        with patch("main.ScoringDataProvider") as mock_provider_class:
            mock_provider = MagicMock()
            mock_provider.fetch_all.return_value = None
            mock_provider_class.return_value = mock_provider

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )

            with pytest.raises(RuntimeError, match="Scoring data not initialized"):
                pipeline._fetch_data()

    def test_write_scores_uses_scoring_data(self) -> None:
        """Test that _write_scores uses entities and spaces from scoring_data."""
        mock_entities = [Entity(id="entity-1", created_at=datetime.now())]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]

        with patch("main.ScoringDataWriter") as mock_writer_class:
            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )
            pipeline.scoring_data = ScoringData(
                entities=mock_entities,
                votes=[],
                users=[],
                spaces=mock_spaces,
            )
            pipeline._write_scores()

            mock_writer_class.assert_called_once_with("postgresql://test/db")
            mock_writer.write_all.assert_called_once_with(mock_entities, mock_spaces)


class TestOutputModes:
    """Tests for OUTPUT_MODE functionality."""

    def test_postgres_mode_only_uses_writer(self) -> None:
        """Test that postgres mode only initializes writer, not emitter."""
        with patch("main.ScoringDataWriter") as mock_writer_class:
            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )

            assert pipeline._writer is not None
            assert pipeline._emitter is None
            mock_writer_class.assert_called_once()

    def test_kafka_mode_only_uses_emitter(self) -> None:
        """Test that kafka mode only initializes emitter, not writer."""
        with patch("main.ScoringDataEmitter") as mock_emitter_class:
            mock_emitter = MagicMock()
            mock_emitter_class.return_value = mock_emitter

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.KAFKA,
                kafka_broker="localhost:9092",
            )

            assert pipeline._writer is None
            assert pipeline._emitter is not None
            mock_emitter_class.assert_called_once()

    def test_all_mode_uses_both_writer_and_emitter(self) -> None:
        """Test that all mode initializes both writer and emitter."""
        with (
            patch("main.ScoringDataWriter") as mock_writer_class,
            patch("main.ScoringDataEmitter") as mock_emitter_class,
        ):
            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            mock_emitter = MagicMock()
            mock_emitter_class.return_value = mock_emitter

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.ALL,
                kafka_broker="localhost:9092",
            )

            assert pipeline._writer is not None
            assert pipeline._emitter is not None
            mock_writer_class.assert_called_once()
            mock_emitter_class.assert_called_once()

    def test_kafka_mode_requires_broker(self) -> None:
        """Test that kafka mode raises error when broker not provided."""
        mock_engine = MagicMock()

        with pytest.raises(ValueError, match="KAFKA_BROKER is required"):
            ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.KAFKA,
                kafka_broker=None,
            )

    def test_all_mode_requires_broker(self) -> None:
        """Test that all mode raises error when broker not provided."""
        mock_engine = MagicMock()

        with pytest.raises(ValueError, match="KAFKA_BROKER is required"):
            ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.ALL,
                kafka_broker=None,
            )

    def test_emitter_receives_kafka_config(self) -> None:
        """Test that emitter is initialized with all Kafka config options."""
        with patch("main.ScoringDataEmitter") as mock_emitter_class:
            mock_emitter = MagicMock()
            mock_emitter_class.return_value = mock_emitter

            mock_engine = MagicMock()
            ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.KAFKA,
                kafka_broker="kafka.example.com:9093",
                kafka_topic="custom.scores",
                kafka_username="user",
                kafka_password="secret",
                kafka_ssl_ca_pem="-----BEGIN CERTIFICATE-----",
                score_batch_size=500,
            )

            mock_emitter_class.assert_called_once_with(
                broker="kafka.example.com:9093",
                topic="custom.scores",
                batch_size=500,
                username="user",
                password="secret",
                ssl_ca_pem="-----BEGIN CERTIFICATE-----",
            )

    def test_write_scores_calls_both_in_all_mode(self) -> None:
        """Test that _write_scores calls both writer and emitter in all mode."""
        mock_entities = [Entity(id="11111111111111111111111111111111", created_at=datetime.now())]
        mock_spaces = [Space(id="22222222222222222222222222222222", created_at=datetime.now())]

        with (
            patch("main.ScoringDataWriter") as mock_writer_class,
            patch("main.ScoringDataEmitter") as mock_emitter_class,
        ):
            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            mock_emitter = MagicMock()
            mock_emitter.flush.return_value = 0
            mock_emitter.messages_produced = 1
            mock_emitter.delivery_errors = 0
            mock_emitter_class.return_value = mock_emitter

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.ALL,
                kafka_broker="localhost:9092",
            )
            pipeline.scoring_data = ScoringData(
                entities=mock_entities,
                votes=[],
                users=[],
                spaces=mock_spaces,
            )
            pipeline._write_scores()

            # Both should be called
            mock_writer.write_all.assert_called_once_with(mock_entities, mock_spaces)
            mock_emitter.emit_all.assert_called_once_with(mock_entities, mock_spaces)
            mock_emitter.flush.assert_called_once()

    def test_write_scores_only_emitter_in_kafka_mode(self) -> None:
        """Test that _write_scores only calls emitter in kafka mode."""
        mock_entities = [Entity(id="11111111111111111111111111111111", created_at=datetime.now())]
        mock_spaces = [Space(id="22222222222222222222222222222222", created_at=datetime.now())]

        with patch("main.ScoringDataEmitter") as mock_emitter_class:
            mock_emitter = MagicMock()
            mock_emitter.flush.return_value = 0
            mock_emitter.messages_produced = 1
            mock_emitter.delivery_errors = 0
            mock_emitter_class.return_value = mock_emitter

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.KAFKA,
                kafka_broker="localhost:9092",
            )
            pipeline.scoring_data = ScoringData(
                entities=mock_entities,
                votes=[],
                users=[],
                spaces=mock_spaces,
            )
            pipeline._write_scores()

            mock_emitter.emit_all.assert_called_once()
            mock_emitter.flush.assert_called_once()

    def test_write_scores_only_writer_in_postgres_mode(self) -> None:
        """Test that _write_scores only calls writer in postgres mode."""
        mock_entities = [Entity(id="entity-1", created_at=datetime.now())]
        mock_spaces = [Space(id="space-1", created_at=datetime.now())]

        with patch("main.ScoringDataWriter") as mock_writer_class:
            mock_writer = MagicMock()
            mock_writer_class.return_value = mock_writer

            mock_engine = MagicMock()
            pipeline = ScoringPipeline(
                "postgresql://test/db",
                mock_engine,
                output_mode=OutputMode.POSTGRES,
            )
            pipeline.scoring_data = ScoringData(
                entities=mock_entities,
                votes=[],
                users=[],
                spaces=mock_spaces,
            )
            pipeline._write_scores()

            mock_writer.write_all.assert_called_once_with(mock_entities, mock_spaces)
