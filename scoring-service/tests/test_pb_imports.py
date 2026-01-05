"""Test that protobuf imports work correctly."""


def test_pb_imports():
    """Verify all protobuf message types can be imported."""
    from src.pb import EntityScore, HermesScoresBatch, PerspectiveScore, SpaceScore

    # Verify classes are importable and are the expected types
    assert EntityScore is not None
    assert PerspectiveScore is not None
    assert SpaceScore is not None
    assert HermesScoresBatch is not None


def test_entity_score_creation():
    """Test EntityScore message creation."""
    from src.pb import EntityScore

    score = EntityScore()
    score.entity_id = b"\x00" * 16
    score.score = 0.95
    score.updated_at = 1704067200

    assert len(score.entity_id) == 16
    assert score.score == 0.95
    assert score.updated_at == 1704067200


def test_perspective_score_creation():
    """Test PerspectiveScore message creation."""
    from src.pb import PerspectiveScore

    score = PerspectiveScore()
    score.entity_id = b"\x01" * 16
    score.space_id = b"\x02" * 16
    score.score = 0.85
    score.updated_at = 1704067200

    assert len(score.entity_id) == 16
    assert len(score.space_id) == 16
    assert score.score == 0.85


def test_space_score_creation():
    """Test SpaceScore message creation."""
    from src.pb import SpaceScore

    score = SpaceScore()
    score.space_id = b"\x03" * 16
    score.score = 0.75
    score.updated_at = 1704067200

    assert len(score.space_id) == 16
    assert score.score == 0.75


def test_hermes_scores_batch_creation():
    """Test HermesScoresBatch message creation with nested scores."""
    from src.pb import EntityScore, HermesScoresBatch, PerspectiveScore, SpaceScore

    batch = HermesScoresBatch()
    batch.computed_at = 1704067200
    batch.batch_sequence = 1
    batch.is_final = True

    # Add entity score
    entity_score = batch.entity_scores.add()
    entity_score.entity_id = b"\x00" * 16
    entity_score.score = 0.95
    entity_score.updated_at = 1704067200

    # Add perspective score
    perspective_score = batch.perspective_scores.add()
    perspective_score.entity_id = b"\x01" * 16
    perspective_score.space_id = b"\x02" * 16
    perspective_score.score = 0.85
    perspective_score.updated_at = 1704067200

    # Add space score
    space_score = batch.space_scores.add()
    space_score.space_id = b"\x03" * 16
    space_score.score = 0.75
    space_score.updated_at = 1704067200

    assert len(batch.entity_scores) == 1
    assert len(batch.perspective_scores) == 1
    assert len(batch.space_scores) == 1
    assert batch.is_final is True
    assert batch.batch_sequence == 1


def test_hermes_scores_batch_serialization():
    """Test HermesScoresBatch can be serialized and deserialized."""
    from src.pb import HermesScoresBatch

    batch = HermesScoresBatch()
    batch.computed_at = 1704067200
    batch.batch_sequence = 42
    batch.is_final = False

    entity_score = batch.entity_scores.add()
    entity_score.entity_id = b"\xde\xad\xbe\xef" + b"\x00" * 12
    entity_score.score = 0.123456
    entity_score.updated_at = 1704067200

    # Serialize
    serialized = batch.SerializeToString()
    assert len(serialized) > 0

    # Deserialize
    deserialized = HermesScoresBatch()
    deserialized.ParseFromString(serialized)

    assert deserialized.computed_at == 1704067200
    assert deserialized.batch_sequence == 42
    assert deserialized.is_final is False
    assert len(deserialized.entity_scores) == 1
    assert deserialized.entity_scores[0].score == 0.123456

