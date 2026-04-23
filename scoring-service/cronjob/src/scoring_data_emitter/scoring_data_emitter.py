"""ScoringDataEmitter module for publishing calculated scores to Kafka."""

import logging
import time
from typing import Callable

from confluent_kafka import Producer

from src.algorithm.models import Entity, Space
from src.pb import HermesScoresBatch

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

        Iterates entities (for global scores), then entity perspectives (for
        local scores), then spaces — appending each item directly into a
        HermesScoresBatch of up to batch_size items. No intermediate lists of
        per-item protobufs are materialized, keeping peak memory bounded to
        one in-flight batch plus the previous "pending" batch awaiting a
        final-flag decision.

        Args:
            entities: List of Entity objects with calculated normalized_score and perspectives.
            spaces: List of Space objects with calculated space_score.
        """
        computed_at = int(time.time())
        t0 = time.time()

        # "pending" holds the last fully-filled batch: we delay producing it
        # until we know whether more items follow, so we can set is_final=True
        # on the actual last batch even when it happens to be exactly full.
        pending: HermesScoresBatch | None = None
        current = self._new_batch(computed_at, batch_sequence=0)
        items_in_current = 0
        batch_sequence = 0

        def rotate_if_full() -> None:
            nonlocal pending, current, items_in_current, batch_sequence
            if items_in_current < self._batch_size:
                return
            if pending is not None:
                self._produce_batch(pending, is_final=False)
            pending = current
            batch_sequence += 1
            current = self._new_batch(computed_at, batch_sequence)
            items_in_current = 0

        for entity in entities:
            rotate_if_full()
            s = current.entity_scores.add()
            s.entity_id = self._uuid_str_to_bytes(entity.id)
            s.score = entity.normalized_score
            s.updated_at = computed_at
            items_in_current += 1

        for entity in entities:
            for perspective in entity.perspectives:
                rotate_if_full()
                s = current.perspective_scores.add()
                s.entity_id = self._uuid_str_to_bytes(perspective.entity_id)
                s.space_id = self._uuid_str_to_bytes(perspective.space_id)
                s.score = perspective.normalized_score
                s.updated_at = computed_at
                items_in_current += 1

        for space in spaces:
            rotate_if_full()
            s = current.space_scores.add()
            s.space_id = self._uuid_str_to_bytes(space.id)
            s.score = space.space_score
            s.updated_at = computed_at
            items_in_current += 1

        # Terminal flush. rotate_if_full is called before each append, so
        # current always has items whenever we added anything at all.
        batches_produced = 0
        if items_in_current > 0:
            if pending is not None:
                self._produce_batch(pending, is_final=False)
            self._produce_batch(current, is_final=True)
            batches_produced = batch_sequence + 1

        elapsed = time.time() - t0
        logger.info(
            "Produced %d batches to topic '%s' in %.1fs",
            batches_produced, self._topic, elapsed,
        )

    def _new_batch(self, computed_at: int, batch_sequence: int) -> HermesScoresBatch:
        """Create an empty HermesScoresBatch with header fields set."""
        batch = HermesScoresBatch()
        batch.computed_at = computed_at
        batch.batch_sequence = batch_sequence
        return batch

    def _produce_batch(self, batch: HermesScoresBatch, is_final: bool) -> None:
        """Serialize a batch and hand it to the Kafka producer."""
        batch.is_final = is_final
        message = batch.SerializeToString()
        self._producer.produce(
            self._topic,
            value=message,
            callback=self._delivery_callback,
        )
        self._producer.poll(0)

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

