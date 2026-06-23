"""Core data models for the geo rankings system."""

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum

import numpy as np

from src.constants import ROOT_SPACE_ID

SPACE_SCORE_DECAY_BASE = 0.8        # Base for space score decay calculation
DISCONNECTED_SPACE_DEPTH = 11       # Depth used for disconnected spaces (no path to root)

__all__ = [
    "VoteType",
    "User",
    "Vote",
    "Perspective",
    "Entity",
    "Space",
    "RankingConfig",
]


class VoteType(Enum):
    """Types of votes that can be cast."""

    UPVOTE = 1
    DOWNVOTE = -1


@dataclass
class User:
    """Represents a user in the system."""

    id: str
    member_spaces: set[str] = field(default_factory=set)
    editor_spaces: set[str] = field(default_factory=set)


@dataclass
class Vote:
    """Represents a vote on an entity."""

    user_id: str
    entity_id: str
    space_id: str
    vote_type: VoteType
    timestamp: datetime
    weight: float = 1.0


@dataclass
class Perspective:
    """Represents a perspective on an entity."""

    id: str
    space_id: str
    entity_id: str
    created_at: datetime
    version: int = 1

    # Computed fields (not stored in DB)
    upvotes: int = 0
    downvotes: int = 0
    raw_score: float = 0.0
    normalized_score: float = 0.0
    contestation_score: float = 0.0

    def calculate_scores(self, votes: list["Vote"]) -> None:
        """Calculate various scores based on votes for this perspective."""
        perspective_votes = [
            v for v in votes if v.entity_id == self.entity_id and v.space_id == self.space_id
        ]

        # Calculate weighted scores
        weighted_upvotes = sum(
            v.weight for v in perspective_votes if v.vote_type == VoteType.UPVOTE
        )
        weighted_downvotes = sum(
            v.weight for v in perspective_votes if v.vote_type == VoteType.DOWNVOTE
        )

        # Maintain integer counts for display
        self.upvotes = sum(1 for vote in perspective_votes if vote.vote_type == VoteType.UPVOTE)
        self.downvotes = sum(1 for vote in perspective_votes if vote.vote_type == VoteType.DOWNVOTE)

        # Apply weighted scores for calculations
        self.raw_score = weighted_upvotes - weighted_downvotes
        self.contestation_score = weighted_upvotes + weighted_downvotes


@dataclass
class Entity:
    """Represents an entity that can be voted on."""

    id: str
    created_at: datetime
    perspective_ids: list[str] = field(default_factory=list)
    perspectives: list[Perspective] = field(default_factory=list)
    version: int = 1

    # Computed fields
    upvotes: int = 0
    downvotes: int = 0
    raw_score: float = 0.0
    normalized_score: float = 0.0
    contestation_score: float = 0.0

    def load_perspectives(self, all_perspectives: list[Perspective]) -> None:
        """Load perspectives from IDs."""
        self.perspectives = [p for p in all_perspectives if p.entity_id == self.id]

    def calculate_scores(self, votes: list[Vote], spaces: list["Space"] | None = None) -> None:
        """Calculate scores for all perspectives."""
        for perspective in self.perspectives:
            perspective.calculate_scores(votes)

        # Aggregate scores across perspectives
        self.upvotes = sum(p.upvotes for p in self.perspectives)
        self.downvotes = sum(p.downvotes for p in self.perspectives)
        self.raw_score = sum(p.raw_score for p in self.perspectives)
        self.contestation_score = sum(p.contestation_score for p in self.perspectives)

        # Calculate normalised score if spaces are provided
        if self.perspectives and spaces is not None:
            self.normalized_score = 0.0
            for perspective in self.perspectives:
                space = next((s for s in spaces if s.id == perspective.space_id), None)
                if space and space.space_score > 0:
                    self.normalized_score += float(perspective.normalized_score) * float(
                        space.space_score
                    )


@dataclass
class Space:
    """Represents a space that contains entities."""

    id: str
    created_at: datetime
    parent_space_id: str | None = None
    subspace_ids: set[str] = field(default_factory=set)

    # Computed fields
    distance_to_root: int | None = None
    space_score: float = 1.0
    member_count: int = 0
    entity_count: int = 0
    activity_score: float = 0.0

    def calculate_space_score(
        self,
        spaces: list["Space"],
        member_counts: dict[str, int],
        entity_counts: dict[str, int],
        root_space_id: str = ROOT_SPACE_ID,
    ) -> None:
        """Calculate space score based on distance from root (GEO).

        ``distance_to_root`` is computed upstream by the topology indexer (atlas) and
        read from the ``scoring_topology_distances`` table. The invariant is: a space
        with no topology entry is not connected to the root, so it is treated as
        disconnected. ``distance_to_root`` should already be set by the data provider;
        ``None`` is handled defensively here as disconnected.
        """
        if self.distance_to_root is None:
            self.distance_to_root = DISCONNECTED_SPACE_DEPTH

        self.space_score = SPACE_SCORE_DECAY_BASE**self.distance_to_root

        self.member_count = member_counts.get(self.id, 0)
        self.entity_count = entity_counts.get(self.id, 0)


@dataclass
class RankingConfig:
    """Configuration for ranking algorithms."""

    # Entity scoring
    use_contestation_score: bool = True
    use_time_decay: bool = False
    time_decay_factor: float = 0.1

    # Space scoring
    include_subspace_votes: bool = False
    use_activity_metrics: bool = False

    # Distance-based vote weighting
    use_distance_weighting: bool = False
    distance_weight_base: float = 0.8
    max_distance: int = 10

    # Normalization
    normalize_scores: bool = True
    normalization_method: str = "z_score"

    # Anti-sybil
    filter_non_members: bool = True

    def __post_init__(self) -> None:
        """Validate configuration after initialisation."""
        if self.use_distance_weighting and self.filter_non_members:
            raise ValueError(
                "Distance weighting and filter_non_members are incompatible. "
                "Distance weighting already handles user proximity by weighting votes based on distance. "
                "Set filter_non_members=False when using distance weighting."
            )

    def normalize_scores_by_space(self, entities: list[Entity], method: str = "z_score") -> None:
        """Normalise perspective scores within each space."""

        # Group perspectives by space
        space_perspectives: dict[str, list[Perspective]] = {}
        for entity in entities:
            for perspective in entity.perspectives:
                if perspective.space_id not in space_perspectives:
                    space_perspectives[perspective.space_id] = []
                space_perspectives[perspective.space_id].append(perspective)

        # Normalise within each space
        for _space_id, perspectives in space_perspectives.items():
            scores = [p.raw_score for p in perspectives]

            if method == "z_score":
                mean_score = float(np.mean(scores))
                std_score = float(np.std(scores))
                if std_score > 0:
                    for perspective in perspectives:
                        perspective.normalized_score = float(
                            (perspective.raw_score - mean_score) / std_score
                        )
                else:
                    for perspective in perspectives:
                        perspective.normalized_score = 0.0

            elif method == "min_max":
                min_score = float(min(scores))
                max_score = float(max(scores))
                score_range = max_score - min_score
                if score_range > 0:
                    for perspective in perspectives:
                        perspective.normalized_score = float(
                            (perspective.raw_score - min_score) / score_range
                        )
                else:
                    for perspective in perspectives:
                        perspective.normalized_score = 0.5

            elif method == "rank":
                # Convert to rank and normalise to [0, 1]
                sorted_perspectives = sorted(perspectives, key=lambda p: p.raw_score, reverse=True)
                for i, perspective in enumerate(sorted_perspectives):
                    # Normalise rank to [0, 1]
                    perspective.normalized_score = (
                        (len(perspectives) - 1 - i) / (len(perspectives) - 1)
                        if len(perspectives) > 1
                        else 0.5
                    )

            elif method == "z_score_sigmoid":
                # Z-score normalisation with sigmoid
                mean_score = float(np.mean(scores))
                std_score = float(np.std(scores))
                if std_score > 0:
                    for perspective in perspectives:
                        z = (perspective.raw_score - mean_score) / std_score
                        perspective.normalized_score = 1 / (1 + np.exp(-z))
                else:
                    for perspective in perspectives:
                        perspective.normalized_score = 0.5

            else:
                raise ValueError(f"Invalid normalisation method: {method}")
