"""Characterization tests for EntityScorer.filter_valid_votes.

These pin the current behavior of member filtering, with focus on the
personal-space edge case.

A personal space has no rows in the `members`/`editors` tables, so the
space owner is not present in any user's `member_spaces`/`editor_spaces`.
When `filter_non_members=True`, `filter_valid_votes` therefore drops *every*
vote cast in a personal space -- including the owner's own vote. That is a
known gap (the owner should arguably count for their own personal space);
these tests document the behavior as-is so a future owner-counts fix has a
guard to flip.
"""

from datetime import datetime

from src.algorithm.models import (
    Entity,
    Perspective,
    RankingConfig,
    User,
    Vote,
    VoteType,
)
from src.algorithm.scoring import EntityScorer

PERSONAL_SPACE_ID = "personal_space_01"
OWNER_ID = "owner_01"
ENTITY_ID = "entity_01"


def _personal_space_scenario(owner_member_spaces: set[str]) -> tuple[list[Vote], list[User], Entity]:
    """Build a single owner voting on an entity within their personal space.

    `owner_member_spaces` controls whether the owner is recorded as a member
    of the personal space -- empty models the real personal-space case (no
    membership rows); a populated set models a space that does have them.
    """
    now = datetime.now()

    owner = User(
        id=OWNER_ID,
        member_spaces=owner_member_spaces,
        editor_spaces=set(),
    )

    entity = Entity(id=ENTITY_ID, created_at=now)
    entity.perspectives = [
        Perspective(
            id="perspective_01",
            space_id=PERSONAL_SPACE_ID,
            entity_id=ENTITY_ID,
            created_at=now,
        ),
    ]

    votes = [
        Vote(
            user_id=OWNER_ID,
            entity_id=ENTITY_ID,
            space_id=PERSONAL_SPACE_ID,
            vote_type=VoteType.UPVOTE,
            timestamp=now,
        ),
    ]

    return votes, [owner], entity


def test_personal_space_owner_vote_dropped_when_filtering() -> None:
    """The gap: with no membership rows, the owner's own vote is filtered out."""
    scorer = EntityScorer(RankingConfig(filter_non_members=True))
    votes, users, entity = _personal_space_scenario(owner_member_spaces=set())

    valid = scorer.filter_valid_votes(votes, users, entity)

    assert valid == []


def test_personal_space_vote_kept_when_filtering_disabled() -> None:
    """The vote is otherwise valid -- only member filtering drops it."""
    scorer = EntityScorer(RankingConfig(filter_non_members=False))
    votes, users, entity = _personal_space_scenario(owner_member_spaces=set())

    valid = scorer.filter_valid_votes(votes, users, entity)

    assert valid == votes


def test_vote_kept_when_owner_is_member_of_the_space() -> None:
    """Positive control: a membership row makes the vote survive filtering.

    Confirms the drop above is specifically caused by missing membership,
    not by some unrelated step in the filter.
    """
    scorer = EntityScorer(RankingConfig(filter_non_members=True))
    votes, users, entity = _personal_space_scenario(
        owner_member_spaces={PERSONAL_SPACE_ID}
    )

    valid = scorer.filter_valid_votes(votes, users, entity)

    assert valid == votes
