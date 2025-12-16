"""API contract tests for RankingEngine.
"""

from datetime import datetime

import pytest

from src.algorithm.models import (
    Entity,
    Perspective,
    RankingConfig,
    Space,
    User,
    Vote,
    VoteType,
)
from src.algorithm.scoring import RankingEngine


# =============================================================================
# Fixtures
# =============================================================================


@pytest.fixture
def sample_spaces() -> list[Space]:
    """Create a sample space hierarchy (root + child spaces)."""
    now = datetime.now()
    root = Space(
        id="space_00",
        created_at=now,
        parent_space_id=None,
    )
    child1 = Space(
        id="space_01",
        created_at=now,
        parent_space_id="space_00",
    )
    child2 = Space(
        id="space_02",
        created_at=now,
        parent_space_id="space_00",
    )
    return [root, child1, child2]


@pytest.fixture
def sample_users(sample_spaces: list[Space]) -> list[User]:
    """Create sample users with member/editor spaces."""
    return [
        User(
            id="user_01",
            member_spaces={sample_spaces[0].id, sample_spaces[1].id},
            editor_spaces=set(),
        ),
        User(
            id="user_02",
            member_spaces={sample_spaces[1].id},
            editor_spaces={sample_spaces[2].id},
        ),
        User(
            id="user_03",
            member_spaces={sample_spaces[0].id, sample_spaces[2].id},
            editor_spaces=set(),
        ),
    ]


@pytest.fixture
def sample_perspectives(sample_spaces: list[Space]) -> list[Perspective]:
    """Create sample perspectives linked to entities and spaces."""
    now = datetime.now()
    return [
        Perspective(
            id="perspective_01",
            space_id=sample_spaces[1].id,
            entity_id="entity_01",
            created_at=now,
        ),
        Perspective(
            id="perspective_02",
            space_id=sample_spaces[2].id,
            entity_id="entity_01",
            created_at=now,
        ),
        Perspective(
            id="perspective_03",
            space_id=sample_spaces[1].id,
            entity_id="entity_02",
            created_at=now,
        ),
    ]


@pytest.fixture
def sample_entities(sample_perspectives: list[Perspective]) -> list[Entity]:
    """Create sample entities with perspectives attached."""
    now = datetime.now()
    entity1 = Entity(
        id="entity_01",
        created_at=now,
    )
    entity1.perspectives = [
        p for p in sample_perspectives if p.entity_id == "entity_01"
    ]

    entity2 = Entity(
        id="entity_02",
        created_at=now,
    )
    entity2.perspectives = [
        p for p in sample_perspectives if p.entity_id == "entity_02"
    ]

    return [entity1, entity2]


@pytest.fixture
def sample_votes(
    sample_users: list[User],
    sample_entities: list[Entity],
    sample_spaces: list[Space],
) -> list[Vote]:
    """Create sample votes from users on entities."""
    now = datetime.now()
    return [
        Vote(
            user_id=sample_users[0].id,
            entity_id=sample_entities[0].id,
            space_id=sample_spaces[1].id,
            vote_type=VoteType.UPVOTE,
            timestamp=now,
        ),
        Vote(
            user_id=sample_users[1].id,
            entity_id=sample_entities[0].id,
            space_id=sample_spaces[1].id,
            vote_type=VoteType.UPVOTE,
            timestamp=now,
        ),
        Vote(
            user_id=sample_users[2].id,
            entity_id=sample_entities[0].id,
            space_id=sample_spaces[2].id,
            vote_type=VoteType.DOWNVOTE,
            timestamp=now,
        ),
        Vote(
            user_id=sample_users[0].id,
            entity_id=sample_entities[1].id,
            space_id=sample_spaces[1].id,
            vote_type=VoteType.UPVOTE,
            timestamp=now,
        ),
    ]


@pytest.fixture
def ranking_engine() -> RankingEngine:
    """Create a RankingEngine with custom RankingConfig."""
    config = RankingConfig(
        use_activity_metrics=True,
        filter_non_members=False,
        normalization_method="min_max",
    )
    return RankingEngine(config)


# =============================================================================
# rank_entities API Tests
# =============================================================================


class TestRankEntitiesAPI:
    """Tests for the rank_entities method contract."""

    def test_rank_entities_returns_list_of_entities(
        self,
        ranking_engine: RankingEngine,
        sample_entities: list[Entity],
        sample_votes: list[Vote],
        sample_users: list[User],
        sample_spaces: list[Space],
    ) -> None:
        """Verify rank_entities returns a list of Entity objects."""
        result = ranking_engine.rank_entities(
            entities=sample_entities,
            votes=sample_votes,
            users=sample_users,
            spaces=sample_spaces,
        )

        assert isinstance(result, list)
        assert len(result) == len(sample_entities)  
        assert all(isinstance(entity, Entity) for entity in result)

    def test_rank_entities_populates_scores(
        self,
        ranking_engine: RankingEngine,
        sample_entities: list[Entity],
        sample_votes: list[Vote],
        sample_users: list[User],
        sample_spaces: list[Space],
    ) -> None:
        """Verify that scores are populated on returned entities."""
        result = ranking_engine.rank_entities(
            entities=sample_entities,
            votes=sample_votes,
            users=sample_users,
            spaces=sample_spaces,
        )

        # Entity scores could be greater than 1
        for entity in result:
            # Check that score fields exist and are numeric
            assert hasattr(entity, "raw_score")
            assert hasattr(entity, "normalized_score")
            assert isinstance(entity.raw_score, (int, float))
            assert isinstance(entity.normalized_score, (int, float))
            assert entity.normalized_score >= 0

    def test_rank_entities_populates_perspective_scores(
        self,
        ranking_engine: RankingEngine,
        sample_entities: list[Entity],
        sample_votes: list[Vote],
        sample_users: list[User],
        sample_spaces: list[Space],
    ) -> None:
        """Verify that perspective scores are calculated for each entity."""
        result = ranking_engine.rank_entities(
            entities=sample_entities,
            votes=sample_votes,
            users=sample_users,
            spaces=sample_spaces,
        )

        for entity in result:
            assert len(entity.perspectives) > 0, "Entity should have perspectives"
            for perspective in entity.perspectives:
                assert isinstance(perspective, Perspective)
                # Check that perspective score fields exist and are numeric
                assert hasattr(perspective, "raw_score")
                assert hasattr(perspective, "normalized_score")
                assert isinstance(perspective.raw_score, (int, float))
                assert isinstance(perspective.normalized_score, (int, float))
                assert perspective.normalized_score >= 0
                assert perspective.normalized_score <= 1

    def test_rank_entities_with_empty_inputs(
        self,
        ranking_engine: RankingEngine,
    ) -> None:
        """Verify API handles empty lists gracefully."""
        result = ranking_engine.rank_entities(
            entities=[],
            votes=[],
            users=[],
            spaces=[],
        )

        assert isinstance(result, list)
        assert len(result) == 0

    def test_rank_entities_without_spaces(
        self,
        ranking_engine: RankingEngine,
        sample_entities: list[Entity],
        sample_votes: list[Vote],
        sample_users: list[User],
    ) -> None:
        """Verify spaces=None works (optional parameter)."""
        result = ranking_engine.rank_entities(
            entities=sample_entities,
            votes=sample_votes,
            users=sample_users,
            spaces=None,
        )

        assert isinstance(result, list)
        assert len(result) == len(sample_entities)
        assert all(isinstance(entity, Entity) for entity in result)


# =============================================================================
# rank_spaces API Tests
# =============================================================================


class TestRankSpacesAPI:
    """Tests for the rank_spaces method contract."""

    def test_rank_spaces_returns_list_of_spaces(
        self,
        ranking_engine: RankingEngine,
        sample_spaces: list[Space],
        sample_entities: list[Entity],
        sample_users: list[User],
    ) -> None:
        """Verify rank_spaces returns a list of Space objects."""
        result = ranking_engine.rank_spaces(
            spaces=sample_spaces,
            entities=sample_entities,
            users=sample_users,
        )

        assert isinstance(result, list)
        assert len(result) == len(sample_spaces)
        assert all(isinstance(space, Space) for space in result)

    def test_rank_spaces_populates_scores(
        self,
        ranking_engine: RankingEngine,
        sample_spaces: list[Space],
        sample_entities: list[Entity],
        sample_users: list[User],
    ) -> None:
        """Verify that scores are populated on returned spaces."""
        result = ranking_engine.rank_spaces(
            spaces=sample_spaces,
            entities=sample_entities,
            users=sample_users,
        )

        for space in result:
            # Check that score fields exist and are numeric
            assert hasattr(space, "space_score")
            assert hasattr(space, "activity_score")
            assert isinstance(space.space_score, (int, float))
            assert isinstance(space.activity_score, (int, float))
            assert space.space_score >= 0
            assert space.space_score <= 1

    def test_rank_spaces_with_empty_inputs(
        self,
        ranking_engine: RankingEngine,
    ) -> None:
        """Verify API handles empty lists gracefully."""
        result = ranking_engine.rank_spaces(
            spaces=[],
            entities=[],
            users=[],
        )

        assert isinstance(result, list)
        assert len(result) == 0

