"""Tests for the precomputed per-space tallies used in space scoring.

These cover ``compute_space_aggregates`` and verify that ``Space.calculate_space_score``
reads ``member_count`` / ``entity_count`` from the required precomputed tallies.
"""

from datetime import datetime

from src.algorithm.models import Entity, Perspective, Space, User
from src.algorithm.scoring import compute_space_aggregates


def _entity(entity_id: str, space_ids: list[str]) -> Entity:
    """Build an entity with one perspective per given space id."""
    now = datetime.now()
    entity = Entity(id=entity_id, created_at=now)
    entity.perspectives = [
        Perspective(
            id=f"{entity_id}-{i}",
            space_id=space_id,
            entity_id=entity_id,
            created_at=now,
        )
        for i, space_id in enumerate(space_ids)
    ]
    return entity


def test_member_counts_dedup_member_and_editor() -> None:
    """A user who is both member and editor of a space is counted once."""
    users = [
        User(id="u1", member_spaces={"s0", "s1"}, editor_spaces=set()),
        User(id="u2", member_spaces={"s1"}, editor_spaces={"s2"}),
        # u3 is BOTH member and editor of s2 -> must count once for s2.
        User(id="u3", member_spaces={"s0", "s2"}, editor_spaces={"s2"}),
    ]

    member_counts, _ = compute_space_aggregates([], users)

    assert member_counts == {"s0": 2, "s1": 2, "s2": 2}


def test_entity_counts_dedup_multiple_perspectives_same_space() -> None:
    """An entity with multiple perspectives in one space counts once for that space."""
    entities = [
        _entity("e1", ["s1", "s2"]),       # in s1 and s2
        _entity("e2", ["s1"]),             # in s1
        _entity("e3", ["s2", "s2", "s2"]),  # 3 perspectives, all in s2 -> count once
    ]

    _, entity_counts = compute_space_aggregates(entities, [])

    assert entity_counts == {"s1": 2, "s2": 2}


def test_empty_inputs_produce_empty_tallies() -> None:
    member_counts, entity_counts = compute_space_aggregates([], [])
    assert member_counts == {}
    assert entity_counts == {}


def test_calculate_space_score_reads_counts_from_tallies() -> None:
    """calculate_space_score populates member/entity counts from the required tallies."""
    now = datetime.now()
    spaces = [
        Space(id="s0", created_at=now, distance_to_root=0),
        Space(id="s1", created_at=now, distance_to_root=1),
        Space(id="s2", created_at=now, distance_to_root=2),
        Space(id="s3", created_at=now, distance_to_root=3),  # absent from both tallies
    ]
    users = [
        User(id="u1", member_spaces={"s0", "s1"}, editor_spaces=set()),
        User(id="u2", member_spaces={"s1"}, editor_spaces={"s2"}),
        User(id="u3", member_spaces={"s0", "s2"}, editor_spaces={"s2"}),
    ]
    entities = [
        _entity("e1", ["s1", "s2"]),
        _entity("e2", ["s1"]),
        _entity("e3", ["s2", "s2"]),  # dedup within s2
    ]

    member_counts, entity_counts = compute_space_aggregates(entities, users)

    for space in spaces:
        space.calculate_space_score(spaces, member_counts, entity_counts)

    by_id = {s.id: s for s in spaces}
    assert by_id["s0"].member_count == 2
    assert by_id["s1"].member_count == 2
    assert by_id["s2"].member_count == 2
    assert by_id["s1"].entity_count == 2
    assert by_id["s2"].entity_count == 2
    # A space absent from the tallies defaults to zero counts.
    assert by_id["s3"].member_count == 0
    assert by_id["s3"].entity_count == 0
