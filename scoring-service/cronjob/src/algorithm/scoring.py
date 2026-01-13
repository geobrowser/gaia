"""Scoring algorithms for entities and spaces."""

import math
from datetime import datetime

import numpy as np

from .models import Entity, RankingConfig, Space, User, Vote

# Constants
HOT_SCORE_TIME_DIVISOR = 45000  # Time divisor for hot score calculation
ACTIVITY_WEIGHT = 0.1           # Activity weight in composite space score (10%)

from .utils import calculate_space_distances


class EntityScorer:
    """Handles entity scoring calculations."""

    def __init__(self, config: RankingConfig):
        self.config = config

    def calculate_raw_score(self, entity: Entity, votes: list[Vote]) -> float:
        """Calculate raw entity score: sum of all perspective scores."""
        # Calculate perspective scores
        for perspective in entity.perspectives:
            perspective.calculate_scores(votes)

        # Aggregate scores
        entity.calculate_scores(votes)
        return entity.raw_score

    def calculate_contestation_score(self, entity: Entity, votes: list[Vote]) -> float:
        """Calculate contestation score: sum of all perspective contestation scores."""
        # Calculate perspective scores
        for perspective in entity.perspectives:
            perspective.calculate_scores(votes)

        # Aggregate contestation scores
        entity.calculate_scores(votes)
        return entity.contestation_score

    def apply_time_decay(self, score: float, age_hours: float) -> float:
        """Apply time decay to score using exponential decay."""
        if not self.config.use_time_decay:
            return score

        # Apply exponential decay
        decayed_score = score * math.exp(-self.config.time_decay_factor * age_hours)
        return decayed_score

    def calculate_hot_score(self, entity: Entity, votes: list[Vote]) -> float:
        """Calculate 'hot' score using logarithmic approach."""
        entity.calculate_scores(votes)

        upvotes = entity.upvotes
        downvotes = entity.downvotes

        vote_difference = upvotes - downvotes
        age_hours = (datetime.now() - entity.created_at).total_seconds() / 3600

        # Apply hot score formula
        hot_score = math.log(max(abs(vote_difference), 1)) + age_hours / HOT_SCORE_TIME_DIVISOR

        return hot_score

    def filter_valid_votes(
        self, votes: list[Vote], users: list[User], entity: Entity
    ) -> list[Vote]:
        """Filter votes to only include valid ones based on anti-sybil rules."""
        relevant_votes = []
        for vote in votes:
            # Find the perspective this vote is for
            perspective = None
            for p in entity.perspectives:
                if p.entity_id == vote.entity_id and p.space_id == vote.space_id:
                    perspective = p
                    break

            if not perspective:
                continue

            # If not filtering non-members, all votes for this entity are valid
            if not self.config.filter_non_members:
                relevant_votes.append(vote)
                continue

            # Find the user who cast this vote
            user = next((u for u in users if u.id == vote.user_id), None)
            if not user:
                continue

            # Check user membership in perspective's space
            if (
                perspective.space_id in user.member_spaces
                or perspective.space_id in user.editor_spaces
            ):
                relevant_votes.append(vote)
                continue

        return relevant_votes

    def apply_distance_weighting(
        self, votes: list[Vote], users: list[User], spaces: list[Space]
    ) -> list[Vote]:
        """Apply distance-based weighting to votes."""
        if not self.config.use_distance_weighting:
            return votes

        # Calculate distance hash table between all spaces
        space_distances = calculate_space_distances(spaces, self.config.max_distance)

        weighted_votes = []
        for vote in votes:
            # Find voting user
            user = next((u for u in users if u.id == vote.user_id), None)
            if not user:
                weighted_votes.append(vote)
                continue

            # Find minimum distance from user's member spaces to vote target space
            min_distance = self.config.max_distance + 1

            # Check both member_spaces and editor_spaces
            user_spaces = set(user.member_spaces).union(set(user.editor_spaces))

            if not user_spaces:
                min_distance = self.config.max_distance
            else:
                for user_space_id in user_spaces:
                    distance_key = (user_space_id, vote.space_id)
                    if distance_key in space_distances:
                        distance = space_distances[distance_key]
                        min_distance = min(min_distance, distance)

            # Apply distance weighting
            if min_distance <= self.config.max_distance:
                distance_weight = self.config.distance_weight_base**min_distance
            else:
                distance_weight = 0.0

            # Create new vote with updated weight
            weighted_vote = Vote(
                user_id=vote.user_id,
                entity_id=vote.entity_id,
                space_id=vote.space_id,
                vote_type=vote.vote_type,
                timestamp=vote.timestamp,
                weight=vote.weight * distance_weight,
            )

            # Include only non-zero weight votes
            if weighted_vote.weight > 0:
                weighted_votes.append(weighted_vote)

        return weighted_votes

    def normalize_scores(self, entities: list[Entity], method: str = "z_score") -> None:
        """Normalise entity scores using specified method."""
        if not self.config.normalize_scores:
            return

        scores = [e.raw_score for e in entities]

        if method == "z_score":
            mean_score = np.mean(scores)
            std_score = np.std(scores)
            if std_score > 0:
                for entity in entities:
                    entity.normalized_score = (entity.raw_score - mean_score) / std_score
            else:
                for entity in entities:
                    entity.normalized_score = 0.0

        elif method == "min_max":
            min_score = min(scores)
            max_score = max(scores)
            score_range = max_score - min_score
            if score_range > 0:
                for entity in entities:
                    entity.normalized_score = (entity.raw_score - min_score) / score_range
            else:
                for entity in entities:
                    entity.normalized_score = 0.5

        elif method == "rank":
            # Convert to rank and normalise to [0, 1]
            sorted_entities = sorted(entities, key=lambda e: e.raw_score, reverse=True)
            for i, entity in enumerate(sorted_entities):
                # Normalise rank to [0, 1]
                entity.normalized_score = (
                    (len(entities) - 1 - i) / (len(entities) - 1) if len(entities) > 1 else 0.5
                )

        elif method == "z_score_sigmoid":
            # Z-score normalisation with sigmoid
            mean_score = np.mean(scores)
            std_score = np.std(scores)
            if std_score > 0:
                for entity in entities:
                    z = (entity.raw_score - mean_score) / std_score
                    entity.normalized_score = 1 / (1 + np.exp(-z))
            else:
                for entity in entities:
                    entity.normalized_score = 0.5

        else:
            raise ValueError(f"Invalid normalisation method: {method}")


class SpaceScorer:
    """Handles space scoring calculations."""

    def __init__(self, config: RankingConfig):
        self.config = config

    def calculate_stake_weight(self, space: Space, users: list[User]) -> float:
        """Calculate stake weight: w_y = sum(x_yk) for all users k staking in space y."""
        # No staking system in current model
        return 0.0

    def calculate_activity_score(self, space: Space, entities: list[Entity]) -> float:
        """Calculate activity score based on perspective scores in the space."""
        activity_score = 0.0
        for entity in entities:
            for perspective in entity.perspectives:
                if perspective.space_id == space.id:
                    activity_score += perspective.raw_score

        return activity_score

    def calculate_composite_space_score(
        self, space: Space, entities: list[Entity], users: list[User]
    ) -> float:
        """Calculate composite space score combining stake and activity."""
        stake_score = self.calculate_stake_weight(space, users)
        activity_score = self.calculate_activity_score(space, entities)

        # Combine scores (can be weighted)
        composite_score = stake_score + (activity_score * ACTIVITY_WEIGHT)

        return composite_score


class RankingEngine:
    """Main ranking engine that orchestrates scoring and ranking."""

    def __init__(self, config: RankingConfig):
        self.config = config
        self.entity_scorer = EntityScorer(config)
        self.space_scorer = SpaceScorer(config)

    def rank_entities(
        self,
        entities: list[Entity],
        votes: list[Vote],
        users: list[User],
        spaces: list[Space] | None = None,
    ) -> list[Entity]:
        """Rank entities by their scores."""

        # Step 0: Calculate space scores if spaces are provided
        if spaces is not None:
            for space in spaces:
                space.calculate_space_score(entities, users, spaces)

        # Step 1: Apply distance-based weighting to votes if enabled
        processed_votes = votes
        if self.config.use_distance_weighting and spaces is not None:
            processed_votes = self.entity_scorer.apply_distance_weighting(votes, users, spaces)

        # Step 2: Calculate raw perspective scores for all entities
        for entity in entities:
            valid_votes = self.entity_scorer.filter_valid_votes(processed_votes, users, entity)

            for perspective in entity.perspectives:
                # Calculate perspective scores using weighted votes
                perspective.calculate_scores(valid_votes)

            # Calculate entity aggregates
            entity.upvotes = sum(p.upvotes for p in entity.perspectives)
            entity.downvotes = sum(p.downvotes for p in entity.perspectives)
            entity.raw_score = sum(p.raw_score for p in entity.perspectives)
            entity.contestation_score = sum(p.contestation_score for p in entity.perspectives)

        # TODO: should updating an entity reduce it's time decay?
        # Step 3: Apply time decay to entity raw scores if enabled
        if self.config.use_time_decay:
            for entity in entities:
                age_hours = (datetime.now() - entity.created_at).total_seconds() / 3600
                entity.raw_score = self.entity_scorer.apply_time_decay(entity.raw_score, age_hours)

        # Step 4: Normalize perspective scores within each space
        if self.config.normalize_scores:
            temp_config = RankingConfig()
            temp_config.normalize_scores = True
            temp_config.normalization_method = self.config.normalization_method
            temp_config.normalize_scores_by_space(entities, self.config.normalization_method)

        # Step 5: Calculate weighted entity normalized scores
        for entity in entities:
            if entity.perspectives and spaces is not None:
                # Calculate weighted normalized score: sum(normalized_perspective_score * space_score)
                entity.normalized_score = 0.0
                for perspective in entity.perspectives:
                    matching_spaces = [s for s in spaces if s.id == perspective.space_id]
                    if matching_spaces and matching_spaces[0].space_score > 0:
                        space = matching_spaces[0]
                        entity.normalized_score += float(perspective.normalized_score) * float(
                            space.space_score
                        )
            else:
                entity.normalized_score = 0

        # Step 6: The weighted scores are already normalized at the perspective level
        # No additional entity-level normalization needed - the weighted sum is the final score

        # Step 7: Sort by normalized score (the final weighted score)
        ranked_entities = sorted(entities, key=lambda e: e.normalized_score, reverse=True)

        return ranked_entities

    def rank_spaces(
        self, spaces: list[Space], entities: list[Entity], users: list[User]
    ) -> list[Space]:
        """Rank spaces by their scores."""
        # Calculate scores for all spaces
        for space in spaces:
            space.calculate_space_score(entities, users, spaces)

            if self.config.use_activity_metrics:
                space.activity_score = self.space_scorer.calculate_activity_score(space, entities)

        # Sort by space score (descending)
        ranked_spaces = sorted(spaces, key=lambda s: s.space_score, reverse=True)

        return ranked_spaces

    def get_top_entities(
        self,
        entities: list[Entity],
        votes: list[Vote],
        users: list[User],
        spaces: list[Space] | None = None,
        limit: int = 10,
    ) -> list[Entity]:
        """Get top N entities by score."""
        ranked_entities = self.rank_entities(entities, votes, users, spaces)
        return ranked_entities[:limit]

    def get_top_spaces(
        self, spaces: list[Space], entities: list[Entity], users: list[User], limit: int = 10
    ) -> list[Space]:
        """Get top N spaces by score."""
        ranked_spaces = self.rank_spaces(spaces, entities, users)
        return ranked_spaces[:limit]
