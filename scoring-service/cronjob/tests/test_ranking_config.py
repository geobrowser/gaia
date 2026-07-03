"""Validation tests for RankingConfig.__post_init__.

Distance weighting and member filtering are two different anti-sybil
mechanisms, and the config treats them as mutually exclusive: enabling both
raises at construction time. These tests pin that invariant.
"""

import pytest

from src.algorithm.models import RankingConfig


def test_distance_weighting_and_member_filter_are_mutually_exclusive() -> None:
    """Enabling both anti-sybil mechanisms at once must raise."""
    with pytest.raises(ValueError, match="incompatible"):
        RankingConfig(
            use_distance_weighting=True,
            filter_non_members=True,
        )


def test_distance_weighting_without_member_filter_is_allowed() -> None:
    config = RankingConfig(
        use_distance_weighting=True,
        filter_non_members=False,
    )
    assert config.use_distance_weighting is True
    assert config.filter_non_members is False


def test_member_filter_without_distance_weighting_is_allowed() -> None:
    config = RankingConfig(
        use_distance_weighting=False,
        filter_non_members=True,
    )
    assert config.filter_non_members is True
    assert config.use_distance_weighting is False


def test_neither_mechanism_enabled_is_allowed() -> None:
    config = RankingConfig(
        use_distance_weighting=False,
        filter_non_members=False,
    )
    assert config.use_distance_weighting is False
    assert config.filter_non_members is False
