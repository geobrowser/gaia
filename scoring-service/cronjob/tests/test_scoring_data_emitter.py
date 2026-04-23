"""Unit tests for ScoringDataEmitter."""

from datetime import datetime
from unittest.mock import MagicMock, call, patch

import pytest

from src.algorithm.models import Entity, Perspective, Space
from src.scoring_data_emitter import ScoringDataEmitter


@pytest.fixture
def mock_producer():
    """Create a mock Kafka Producer."""
    with patch("src.scoring_data_emitter.scoring_data_emitter.Producer") as mock:
        producer_instance = MagicMock()
        mock.return_value = producer_instance
        yield producer_instance


@pytest.fixture
def emitter(mock_producer):
    """Create a ScoringDataEmitter with mocked producer."""
    return ScoringDataEmitter(
        broker="localhost:9092",
        topic="test.scores",
        batch_size=100,
    )


@pytest.fixture
def sample_entities():
    """Create sample entities with perspectives."""
    now = datetime.now()

    entity1 = Entity(id="11111111-1111-1111-1111-111111111111", created_at=now)
    entity1.normalized_score = 0.85
    entity1.perspectives = [
        Perspective(
            id="aaaa1111-1111-1111-1111-111111111111",
            space_id="22222222-2222-2222-2222-222222222222",
            entity_id="11111111-1111-1111-1111-111111111111",
            created_at=now,
        ),
    ]
    entity1.perspectives[0].normalized_score = 0.9

    entity2 = Entity(id="33333333-3333-3333-3333-333333333333", created_at=now)
    entity2.normalized_score = 0.65
    entity2.perspectives = [
        Perspective(
            id="bbbb2222-2222-2222-2222-222222222222",
            space_id="22222222-2222-2222-2222-222222222222",
            entity_id="33333333-3333-3333-3333-333333333333",
            created_at=now,
        ),
        Perspective(
            id="cccc3333-3333-3333-3333-333333333333",
            space_id="44444444-4444-4444-4444-444444444444",
            entity_id="33333333-3333-3333-3333-333333333333",
            created_at=now,
        ),
    ]
    entity2.perspectives[0].normalized_score = 0.7
    entity2.perspectives[1].normalized_score = 0.6

    return [entity1, entity2]


@pytest.fixture
def sample_spaces():
    """Create sample spaces."""
    now = datetime.now()

    space1 = Space(id="22222222-2222-2222-2222-222222222222", created_at=now)
    space1.space_score = 0.8

    space2 = Space(id="44444444-4444-4444-4444-444444444444", created_at=now)
    space2.space_score = 0.64

    return [space1, space2]


class TestScoringDataEmitterInit:
    """Tests for ScoringDataEmitter initialization."""

    def test_init_basic_config(self):
        """Test initialization with basic configuration."""
        with patch("src.scoring_data_emitter.scoring_data_emitter.Producer") as mock_producer:
            emitter = ScoringDataEmitter(
                broker="localhost:9092",
                topic="test.topic",
                batch_size=500,
            )

            # Verify producer was created with correct config
            config = mock_producer.call_args[0][0]
            assert config["bootstrap.servers"] == "localhost:9092"
            assert config["compression.type"] == "zstd"
            assert "security.protocol" not in config

    def test_init_with_sasl_ssl(self):
        """Test initialization with SASL/SSL configuration."""
        with patch("src.scoring_data_emitter.scoring_data_emitter.Producer") as mock_producer:
            emitter = ScoringDataEmitter(
                broker="kafka.example.com:9093",
                topic="secure.topic",
                username="user",
                password="secret",
                ssl_ca_pem="-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----",
            )

            config = mock_producer.call_args[0][0]
            assert config["security.protocol"] == "SASL_SSL"
            assert config["sasl.mechanism"] == "PLAIN"
            assert config["sasl.username"] == "user"
            assert config["sasl.password"] == "secret"
            assert "-----BEGIN CERTIFICATE-----" in config["ssl.ca.pem"]

class TestUuidConversion:
    """Tests for UUID string to bytes conversion."""

    def test_uuid_with_hyphens(self, emitter):
        """Test UUID conversion with hyphens."""
        uuid_str = "11111111-1111-1111-1111-111111111111"
        result = emitter._uuid_str_to_bytes(uuid_str)

        assert len(result) == 16
        assert result == bytes.fromhex("11111111111111111111111111111111")

    def test_uuid_without_hyphens(self, emitter):
        """Test UUID conversion without hyphens."""
        uuid_str = "22222222222222222222222222222222"
        result = emitter._uuid_str_to_bytes(uuid_str)

        assert len(result) == 16
        assert result == bytes.fromhex("22222222222222222222222222222222")


class TestEmitAll:
    """Tests for emit_all method."""

    def test_emit_all_single_batch(self, emitter, mock_producer, sample_entities, sample_spaces):
        """Test emitting scores in a single batch."""
        emitter.emit_all(sample_entities, sample_spaces)

        # Should produce one message (all items fit in batch_size=100)
        assert mock_producer.produce.call_count == 1

        # Verify topic (first positional arg)
        call_args = mock_producer.produce.call_args
        assert call_args[0][0] == "test.scores"

        # Verify message is bytes (serialized protobuf)
        message = call_args.kwargs["value"]
        assert isinstance(message, bytes)
        assert len(message) > 0

        # Verify poll was called
        mock_producer.poll.assert_called()

    def test_emit_all_multiple_batches(self, mock_producer):
        """Test emitting scores across multiple batches."""
        # Create emitter with small batch size
        emitter = ScoringDataEmitter(
            broker="localhost:9092",
            topic="test.scores",
            batch_size=2,  # Very small to force multiple batches
        )

        now = datetime.now()
        entities = [
            Entity(id=f"{i:032x}", created_at=now)
            for i in range(5)
        ]
        for e in entities:
            e.normalized_score = 0.5
            e.perspectives = []

        spaces = [
            Space(id=f"{i:032x}", created_at=now)
            for i in range(100, 103)
        ]
        for s in spaces:
            s.space_score = 0.5

        emitter.emit_all(entities, spaces)

        # 5 entities + 3 spaces = 8 items, batch_size=2 => 4 batches
        assert mock_producer.produce.call_count == 4

    def test_emit_all_sets_is_final_on_last_batch(self, mock_producer):
        """Test that is_final is set correctly."""
        from src.pb import HermesScoresBatch

        emitter = ScoringDataEmitter(
            broker="localhost:9092",
            topic="test.scores",
            batch_size=2,
        )

        now = datetime.now()
        entities = [Entity(id=f"{i:032x}", created_at=now) for i in range(3)]
        for e in entities:
            e.normalized_score = 0.5
            e.perspectives = []

        emitter.emit_all(entities, [])

        # Deserialize the last message
        calls = mock_producer.produce.call_args_list
        last_message = calls[-1].kwargs["value"]

        batch = HermesScoresBatch()
        batch.ParseFromString(last_message)
        assert batch.is_final is True

        # Check first batch is not final
        first_message = calls[0].kwargs["value"]
        first_batch = HermesScoresBatch()
        first_batch.ParseFromString(first_message)
        assert first_batch.is_final is False

class TestEmittedContent:
    """Verify the bytes we produce to Kafka via the public emit_all interface.

    These tests deserialize the serialized protobuf payloads and assert the
    field values — the same coverage TestPrepareScores used to provide, but
    through the public interface so the tests survive internal refactors.
    """

    def _deserialize_batches(self, mock_producer):
        from src.pb import HermesScoresBatch

        batches = []
        for call_args in mock_producer.produce.call_args_list:
            batch = HermesScoresBatch()
            batch.ParseFromString(call_args.kwargs["value"])
            batches.append(batch)
        return batches

    def test_emit_all_encodes_entity_scores(self, emitter, mock_producer, sample_entities, sample_spaces):
        emitter.emit_all(sample_entities, sample_spaces)
        batches = self._deserialize_batches(mock_producer)

        entity_scores = [s for b in batches for s in b.entity_scores]
        assert len(entity_scores) == 2

        assert entity_scores[0].entity_id == bytes.fromhex("11111111111111111111111111111111")
        assert entity_scores[0].score == pytest.approx(0.85)
        assert entity_scores[0].updated_at > 0

        assert entity_scores[1].entity_id == bytes.fromhex("33333333333333333333333333333333")
        assert entity_scores[1].score == pytest.approx(0.65)

    def test_emit_all_encodes_perspective_scores(self, emitter, mock_producer, sample_entities, sample_spaces):
        emitter.emit_all(sample_entities, sample_spaces)
        batches = self._deserialize_batches(mock_producer)

        persp_scores = [s for b in batches for s in b.perspective_scores]
        assert len(persp_scores) == 3

        assert persp_scores[0].entity_id == bytes.fromhex("11111111111111111111111111111111")
        assert persp_scores[0].space_id == bytes.fromhex("22222222222222222222222222222222")
        assert persp_scores[0].score == pytest.approx(0.9)

        assert persp_scores[1].entity_id == bytes.fromhex("33333333333333333333333333333333")
        assert persp_scores[1].space_id == bytes.fromhex("22222222222222222222222222222222")
        assert persp_scores[1].score == pytest.approx(0.7)

        assert persp_scores[2].space_id == bytes.fromhex("44444444444444444444444444444444")
        assert persp_scores[2].score == pytest.approx(0.6)

    def test_emit_all_encodes_space_scores(self, emitter, mock_producer, sample_entities, sample_spaces):
        emitter.emit_all(sample_entities, sample_spaces)
        batches = self._deserialize_batches(mock_producer)

        space_scores = [s for b in batches for s in b.space_scores]
        assert len(space_scores) == 2

        assert space_scores[0].space_id == bytes.fromhex("22222222222222222222222222222222")
        assert space_scores[0].score == pytest.approx(0.8)

        assert space_scores[1].space_id == bytes.fromhex("44444444444444444444444444444444")
        assert space_scores[1].score == pytest.approx(0.64)

    def test_emit_all_empty_inputs_produces_nothing(self, emitter, mock_producer):
        emitter.emit_all([], [])
        assert mock_producer.produce.call_count == 0

    def test_emit_all_sequence_is_monotonic(self, mock_producer):
        from src.pb import HermesScoresBatch

        emitter = ScoringDataEmitter(broker="localhost:9092", topic="test.scores", batch_size=2)
        now = datetime.now()
        entities = [Entity(id=f"{i:032x}", created_at=now) for i in range(5)]
        for e in entities:
            e.normalized_score = 0.5
            e.perspectives = []

        emitter.emit_all(entities, [])

        sequences = []
        computed_ats = set()
        for call_args in mock_producer.produce.call_args_list:
            batch = HermesScoresBatch()
            batch.ParseFromString(call_args.kwargs["value"])
            sequences.append(batch.batch_sequence)
            computed_ats.add(batch.computed_at)

        assert sequences == list(range(len(sequences)))
        assert len(computed_ats) == 1  # same computed_at across all batches

    def test_emit_all_last_batch_exactly_full_is_final(self, mock_producer):
        """Edge case: when the last item exactly fills a batch, that batch must be is_final=True."""
        from src.pb import HermesScoresBatch

        emitter = ScoringDataEmitter(broker="localhost:9092", topic="test.scores", batch_size=2)
        now = datetime.now()
        # Exactly 4 entities with batch_size=2 -> 2 batches, second exactly full
        entities = [Entity(id=f"{i:032x}", created_at=now) for i in range(4)]
        for e in entities:
            e.normalized_score = 0.5
            e.perspectives = []

        emitter.emit_all(entities, [])

        assert mock_producer.produce.call_count == 2
        last_message = mock_producer.produce.call_args_list[-1].kwargs["value"]
        last_batch = HermesScoresBatch()
        last_batch.ParseFromString(last_message)
        assert last_batch.is_final is True
        assert len(last_batch.entity_scores) == 2  # exactly full

        first_message = mock_producer.produce.call_args_list[0].kwargs["value"]
        first_batch = HermesScoresBatch()
        first_batch.ParseFromString(first_message)
        assert first_batch.is_final is False


class TestDeliveryCallback:
    """Tests for delivery callback handling."""

    def test_delivery_success(self, emitter):
        """Test successful delivery increments counter."""
        assert emitter.messages_produced == 0

        emitter._delivery_callback(None, MagicMock())

        assert emitter.messages_produced == 1
        assert emitter.delivery_errors == 0

    def test_delivery_failure(self, emitter):
        """Test failed delivery increments error counter."""
        assert emitter.delivery_errors == 0

        emitter._delivery_callback(Exception("Broker unavailable"), MagicMock())

        assert emitter.delivery_errors == 1
        assert emitter.messages_produced == 0


class TestFlush:
    """Tests for flush method."""

    def test_flush_success(self, emitter, mock_producer):
        """Test successful flush."""
        mock_producer.flush.return_value = 0

        remaining = emitter.flush(timeout=10.0)

        assert remaining == 0
        mock_producer.flush.assert_called_once_with(10.0)

    def test_flush_with_remaining(self, emitter, mock_producer):
        """Test flush with messages remaining."""
        mock_producer.flush.return_value = 5

        remaining = emitter.flush()

        assert remaining == 5

    def test_close_calls_flush(self, emitter, mock_producer):
        """Test close calls flush."""
        mock_producer.flush.return_value = 0

        emitter.close()

        mock_producer.flush.assert_called_once()

