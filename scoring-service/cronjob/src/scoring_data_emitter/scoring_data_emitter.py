"""ScoringDataEmitter module for publishing calculated scores to Kafka."""

import logging
import time
from typing import Callable

from confluent_kafka import Producer

from src.algorithm.models import Entity, Space
from src.pb import EntityScore, HermesScoresBatch, PerspectiveScore, SpaceScore

logger = logging.getLogger(__name__)


class ScoringDataEmitter:
    """Publishes calculated scores to Kafka using protobuf serialization."""

    def __init__(
        self,
        broker: str,
        topic: str = "curation.scores",
        batch_size: int = 1000,
        username: str | None = None,
        password: str | None = None,
        ssl_ca_pem: str | None = None,
    ):
        """Initialize the ScoringDataEmitter with Kafka configuration.

        Args:
            broker: Kafka broker address (e.g., "localhost:9092").
            topic: Kafka topic to publish scores to.
            batch_size: Maximum number of scores per HermesScoresBatch message.
            username: SASL username for authentication (optional).
            password: SASL password for authentication (optional).
            ssl_ca_pem: Custom CA certificate PEM content (optional).
        """
        self._topic = topic
        self._batch_size = batch_size

        # Build Kafka producer config
        config: dict[str, str | Callable[[str, bytes], bytes]] = {
            "bootstrap.servers": broker,
            "compression.type": "zstd",
            "linger.ms": "100",
            "batch.num.messages": "10000",
            "queue.buffering.max.messages": "100000",
        }

        # Add SASL/SSL configuration if credentials provided
        if username and password:
            config.update({
                "security.protocol": "SASL_SSL",
                "sasl.mechanism": "PLAIN",
                "sasl.username": username,
                "sasl.password": password,
            })

            if ssl_ca_pem:
                config["ssl.ca.pem"] = ssl_ca_pem

        self._producer = Producer(config)
        self._messages_produced = 0
        self._delivery_errors = 0

    def _delivery_callback(self, err: Exception | None, msg: object) -> None:
        """Callback for message delivery reports."""
        if err is not None:
            logger.error(f"Message delivery failed: {err}")
            self._delivery_errors += 1
        else:
            self._messages_produced += 1

    def _uuid_str_to_bytes(self, uuid_str: str) -> bytes:
        """Convert UUID string to 16-byte representation.

        Args:
            uuid_str: UUID string (with or without hyphens).

        Returns:
            16-byte representation of the UUID.
        """
        # Remove hyphens and convert to bytes
        hex_str = uuid_str.replace("-", "")
        return bytes.fromhex(hex_str)

    def emit_all(self, entities: list[Entity], spaces: list[Space]) -> None:
        """Emit all scores to Kafka in batches.

        Scores are grouped into HermesScoresBatch messages respecting the
        configured batch_size. Each batch includes entity scores, perspective
        scores, and space scores.

        Args:
            entities: List of Entity objects with calculated normalized_score and perspectives.
            spaces: List of Space objects with calculated space_score.
        """
        computed_at = int(time.time())

        # Prepare all scores
        entity_scores = self._prepare_entity_scores(entities, computed_at)
        perspective_scores = self._prepare_perspective_scores(entities, computed_at)
        space_scores = self._prepare_space_scores(spaces, computed_at)

        logger.info(
            f"Preparing to emit scores: {len(entity_scores)} entities, "
            f"{len(perspective_scores)} perspectives, {len(space_scores)} spaces"
        )

        # Calculate total batches needed
        total_items = len(entity_scores) + len(perspective_scores) + len(space_scores)
        total_batches = max(1, (total_items + self._batch_size - 1) // self._batch_size)

        # Emit in batches
        batch_sequence = 0
        entity_idx = 0
        perspective_idx = 0
        space_idx = 0

        while entity_idx < len(entity_scores) or \
              perspective_idx < len(perspective_scores) or \
              space_idx < len(space_scores):

            batch = HermesScoresBatch()
            batch.computed_at = computed_at
            batch.batch_sequence = batch_sequence

            items_in_batch = 0

            # Add entity scores to batch
            while entity_idx < len(entity_scores) and items_in_batch < self._batch_size:
                score = batch.entity_scores.add()
                src = entity_scores[entity_idx]
                score.entity_id = src.entity_id
                score.score = src.score
                score.updated_at = src.updated_at
                entity_idx += 1
                items_in_batch += 1

            # Add perspective scores to batch
            while perspective_idx < len(perspective_scores) and items_in_batch < self._batch_size:
                score = batch.perspective_scores.add()
                src = perspective_scores[perspective_idx]
                score.entity_id = src.entity_id
                score.space_id = src.space_id
                score.score = src.score
                score.updated_at = src.updated_at
                perspective_idx += 1
                items_in_batch += 1

            # Add space scores to batch
            while space_idx < len(space_scores) and items_in_batch < self._batch_size:
                score = batch.space_scores.add()
                src = space_scores[space_idx]
                score.space_id = src.space_id
                score.score = src.score
                score.updated_at = src.updated_at
                space_idx += 1
                items_in_batch += 1

            # Mark final batch
            is_final = (
                entity_idx >= len(entity_scores) and
                perspective_idx >= len(perspective_scores) and
                space_idx >= len(space_scores)
            )
            batch.is_final = is_final

            # Serialize and produce
            message = batch.SerializeToString()
            self._producer.produce(
                self._topic,
                value=message,
                callback=self._delivery_callback,
            )

            # Poll to handle delivery callbacks
            self._producer.poll(0)

            batch_sequence += 1

            logger.debug(
                f"Produced batch {batch_sequence}/{total_batches} "
                f"({len(batch.entity_scores)} entities, "
                f"{len(batch.perspective_scores)} perspectives, "
                f"{len(batch.space_scores)} spaces, "
                f"is_final={is_final})"
            )

        logger.info(f"Produced {batch_sequence} batches to topic '{self._topic}'")

    def _prepare_entity_scores(
        self, entities: list[Entity], updated_at: int
    ) -> list[EntityScore]:
        """Prepare EntityScore messages from entities."""
        scores = []
        for entity in entities:
            score = EntityScore()
            score.entity_id = self._uuid_str_to_bytes(entity.id)
            score.score = entity.normalized_score
            score.updated_at = updated_at
            scores.append(score)
        return scores

    def _prepare_perspective_scores(
        self, entities: list[Entity], updated_at: int
    ) -> list[PerspectiveScore]:
        """Prepare PerspectiveScore messages from entity perspectives."""
        scores = []
        for entity in entities:
            for perspective in entity.perspectives:
                score = PerspectiveScore()
                score.entity_id = self._uuid_str_to_bytes(perspective.entity_id)
                score.space_id = self._uuid_str_to_bytes(perspective.space_id)
                score.score = perspective.normalized_score
                score.updated_at = updated_at
                scores.append(score)
        return scores

    def _prepare_space_scores(
        self, spaces: list[Space], updated_at: int
    ) -> list[SpaceScore]:
        """Prepare SpaceScore messages from spaces."""
        scores = []
        for space in spaces:
            score = SpaceScore()
            score.space_id = self._uuid_str_to_bytes(space.id)
            score.score = space.space_score
            score.updated_at = updated_at
            scores.append(score)
        return scores

    def flush(self, timeout: float = 30.0) -> int:
        """Flush pending messages and wait for delivery.

        Args:
            timeout: Maximum time to wait for delivery in seconds.

        Returns:
            Number of messages still in queue (0 means all delivered).
        """
        remaining = self._producer.flush(timeout)

        if remaining > 0:
            logger.warning(f"{remaining} messages still in queue after flush timeout")
        else:
            logger.info(
                f"All messages delivered. Produced: {self._messages_produced}, "
                f"Errors: {self._delivery_errors}"
            )

        return remaining

    def close(self) -> None:
        """Flush and close the producer."""
        self.flush()

    @property
    def messages_produced(self) -> int:
        """Number of messages successfully produced."""
        return self._messages_produced

    @property
    def delivery_errors(self) -> int:
        """Number of delivery errors encountered."""
        return self._delivery_errors

