"""Tests that z_score_sigmoid normalization produces scores in [0, 1]."""

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
from src.constants import ROOT_SPACE_ID


def _make_engine() -> RankingEngine:
    config = RankingConfig(
        normalize_scores=True,
        normalization_method="z_score_sigmoid",
        filter_non_members=False,
        use_activity_metrics=False,
        use_distance_weighting=False,
    )
    return RankingEngine(config)


def _make_spaces(n: int = 3) -> list[Space]:
    """Create root + n-1 child spaces."""
    now = datetime.now()
    root = Space(id=ROOT_SPACE_ID, created_at=now, parent_space_id=None)
    children = [
        Space(id=f"space_{i:02d}", created_at=now, parent_space_id=ROOT_SPACE_ID)
        for i in range(1, n)
    ]
    return [root] + children


def _make_users(n: int, spaces: list[Space]) -> list[User]:
    space_ids = {s.id for s in spaces}
    return [
        User(id=f"user_{i:02d}", member_spaces=space_ids, editor_spaces=set())
        for i in range(n)
    ]


class TestZScoreSigmoidNormalization:
    """Verify that z_score_sigmoid normalization keeps all scores in [0, 1]."""

    def test_perspective_scores_bounded_basic(self) -> None:
        """Basic scenario: a few entities with mixed upvotes/downvotes."""
        now = datetime.now()
        spaces = _make_spaces(3)
        users = _make_users(5, spaces)

        # Build entities with perspectives in child spaces
        entities = []
        perspectives_all = []
        for i in range(5):
            perspectives = [
                Perspective(
                    id=f"entity_{i:02d}_{spaces[1].id}",
                    space_id=spaces[1].id,
                    entity_id=f"entity_{i:02d}",
                    created_at=now,
                ),
                Perspective(
                    id=f"entity_{i:02d}_{spaces[2].id}",
                    space_id=spaces[2].id,
                    entity_id=f"entity_{i:02d}",
                    created_at=now,
                ),
            ]
            entity = Entity(id=f"entity_{i:02d}", created_at=now)
            entity.perspectives = perspectives
            entities.append(entity)
            perspectives_all.extend(perspectives)

        # Create varied votes: entity_00 gets lots of upvotes, entity_04 gets downvotes
        votes = []
        vote_patterns = [
            (0, VoteType.UPVOTE, 5),   # entity_00: 5 upvotes
            (1, VoteType.UPVOTE, 3),   # entity_01: 3 upvotes
            (2, VoteType.UPVOTE, 1),   # entity_02: 1 upvote, 1 downvote
            (2, VoteType.DOWNVOTE, 1),
            (3, VoteType.DOWNVOTE, 2), # entity_03: 2 downvotes
            (4, VoteType.DOWNVOTE, 4), # entity_04: 4 downvotes
        ]
        for entity_idx, vote_type, count in vote_patterns:
            for j in range(count):
                user = users[j % len(users)]
                for space in spaces[1:]:  # votes in both child spaces
                    votes.append(
                        Vote(
                            user_id=user.id,
                            entity_id=f"entity_{entity_idx:02d}",
                            space_id=space.id,
                            vote_type=vote_type,
                            timestamp=now,
                        )
                    )

        engine = _make_engine()
        ranked = engine.rank_entities(entities, votes, users, spaces)

        for entity in ranked:
            for perspective in entity.perspectives:
                assert 0 <= perspective.normalized_score <= 1, (
                    f"Perspective {perspective.id} normalized_score "
                    f"{perspective.normalized_score} not in [0, 1]"
                )

    def test_perspective_scores_bounded_single_entity(self) -> None:
        """Single entity per space: all perspectives should get 0.5 (no variance)."""
        now = datetime.now()
        spaces = _make_spaces(2)
        users = _make_users(2, spaces)

        entity = Entity(id="entity_solo", created_at=now)
        entity.perspectives = [
            Perspective(
                id="solo_perspective",
                space_id=spaces[1].id,
                entity_id="entity_solo",
                created_at=now,
            )
        ]

        votes = [
            Vote(
                user_id=users[0].id,
                entity_id="entity_solo",
                space_id=spaces[1].id,
                vote_type=VoteType.UPVOTE,
                timestamp=now,
            )
        ]

        engine = _make_engine()
        ranked = engine.rank_entities([entity], votes, users, spaces)

        # With only one perspective in the space, std=0 → score should be 0.5
        assert ranked[0].perspectives[0].normalized_score == pytest.approx(0.5)

    def test_perspective_scores_bounded_all_same_votes(self) -> None:
        """All entities receive identical votes: std=0 → all get 0.5."""
        now = datetime.now()
        spaces = _make_spaces(2)
        users = _make_users(3, spaces)

        entities = []
        votes = []
        for i in range(4):
            entity = Entity(id=f"entity_{i}", created_at=now)
            entity.perspectives = [
                Perspective(
                    id=f"p_{i}",
                    space_id=spaces[1].id,
                    entity_id=f"entity_{i}",
                    created_at=now,
                )
            ]
            entities.append(entity)

            # Each entity gets exactly 2 upvotes
            for j in range(2):
                votes.append(
                    Vote(
                        user_id=users[j].id,
                        entity_id=f"entity_{i}",
                        space_id=spaces[1].id,
                        vote_type=VoteType.UPVOTE,
                        timestamp=now,
                    )
                )

        engine = _make_engine()
        ranked = engine.rank_entities(entities, votes, users, spaces)

        for entity in ranked:
            for p in entity.perspectives:
                assert p.normalized_score == pytest.approx(0.5), (
                    f"Expected 0.5 for uniform scores, got {p.normalized_score}"
                )

    def test_perspective_scores_bounded_extreme_skew(self) -> None:
        """Extreme vote distribution: one entity gets 50 upvotes, rest get 0."""
        now = datetime.now()
        spaces = _make_spaces(2)
        users = _make_users(50, spaces)

        entities = []
        for i in range(10):
            entity = Entity(id=f"entity_{i}", created_at=now)
            entity.perspectives = [
                Perspective(
                    id=f"p_{i}",
                    space_id=spaces[1].id,
                    entity_id=f"entity_{i}",
                    created_at=now,
                )
            ]
            entities.append(entity)

        # Only entity_0 gets 50 upvotes, the rest get none
        votes = [
            Vote(
                user_id=f"user_{j:02d}",
                entity_id="entity_0",
                space_id=spaces[1].id,
                vote_type=VoteType.UPVOTE,
                timestamp=now,
            )
            for j in range(50)
        ]

        engine = _make_engine()
        ranked = engine.rank_entities(entities, votes, users, spaces)

        for entity in ranked:
            for p in entity.perspectives:
                assert 0 <= p.normalized_score <= 1, (
                    f"Perspective {p.id} score {p.normalized_score} out of range"
                )

    def test_perspective_scores_bounded_only_downvotes(self) -> None:
        """All entities receive only downvotes — scores should still be in [0, 1]."""
        now = datetime.now()
        spaces = _make_spaces(2)
        users = _make_users(5, spaces)

        entities = []
        votes = []
        for i in range(5):
            entity = Entity(id=f"entity_{i}", created_at=now)
            entity.perspectives = [
                Perspective(
                    id=f"p_{i}",
                    space_id=spaces[1].id,
                    entity_id=f"entity_{i}",
                    created_at=now,
                )
            ]
            entities.append(entity)

            # Each entity gets i+1 downvotes
            for j in range(i + 1):
                votes.append(
                    Vote(
                        user_id=users[j].id,
                        entity_id=f"entity_{i}",
                        space_id=spaces[1].id,
                        vote_type=VoteType.DOWNVOTE,
                        timestamp=now,
                    )
                )

        engine = _make_engine()
        ranked = engine.rank_entities(entities, votes, users, spaces)

        for entity in ranked:
            for p in entity.perspectives:
                assert 0 <= p.normalized_score <= 1, (
                    f"Perspective {p.id} score {p.normalized_score} out of range"
                )

    def test_perspective_scores_bounded_large_scale(self) -> None:
        """Larger scale: 100 entities, random-ish vote distribution."""
        now = datetime.now()
        spaces = _make_spaces(4)  # root + 3 child spaces
        users = _make_users(20, spaces)

        entities = []
        votes = []
        for i in range(100):
            # Distribute entities across child spaces
            space = spaces[1 + (i % 3)]
            entity = Entity(id=f"entity_{i}", created_at=now)
            entity.perspectives = [
                Perspective(
                    id=f"p_{i}",
                    space_id=space.id,
                    entity_id=f"entity_{i}",
                    created_at=now,
                )
            ]
            entities.append(entity)

            # Vary votes: some get upvotes, some downvotes, some mixed
            upvote_count = (i * 7 + 3) % 10  # deterministic pseudo-random
            downvote_count = (i * 3 + 1) % 6
            for j in range(upvote_count):
                votes.append(
                    Vote(
                        user_id=users[j % len(users)].id,
                        entity_id=f"entity_{i}",
                        space_id=space.id,
                        vote_type=VoteType.UPVOTE,
                        timestamp=now,
                    )
                )
            for j in range(downvote_count):
                votes.append(
                    Vote(
                        user_id=users[(j + upvote_count) % len(users)].id,
                        entity_id=f"entity_{i}",
                        space_id=space.id,
                        vote_type=VoteType.DOWNVOTE,
                        timestamp=now,
                    )
                )

        engine = _make_engine()
        ranked = engine.rank_entities(entities, votes, users, spaces)

        for entity in ranked:
            for p in entity.perspectives:
                assert 0 <= p.normalized_score <= 1, (
                    f"Perspective {p.id} score {p.normalized_score} out of range"
                )
