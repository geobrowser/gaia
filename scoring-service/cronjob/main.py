"""Main entry point for the scoring service."""

import logging
import os
import sys
from enum import Enum

import sentry_sdk
from dotenv import load_dotenv
from sentry_sdk.integrations.logging import LoggingIntegration

from src.algorithm.models import RankingConfig
from src.algorithm.scoring import RankingEngine
from src.scoring_data_emitter import ScoringDataEmitter
from src.scoring_data_provider import ScoringDataProvider
from src.scoring_data_provider.scoring_data_provider import ScoringData
from src.scoring_data_writer import ScoringDataWriter

logger = logging.getLogger(__name__)


class OutputMode(Enum):
    """Output mode for the scoring pipeline."""

    POSTGRES = "postgres"  # Write to PostgreSQL only
    KAFKA = "kafka"  # Publish to Kafka only
    ALL = "all"  # Write to PostgreSQL AND publish to Kafka


class ScoringPipeline:
    """Scoring pipeline with configurable output modes."""

    def __init__(
        self,
        database_url: str,
        engine: RankingEngine,
        output_mode: OutputMode = OutputMode.ALL,
        kafka_broker: str | None = None,
        kafka_topic: str = "curation.scores",
        kafka_username: str | None = None,
        kafka_password: str | None = None,
        kafka_ssl_ca_pem: str | None = None,
        score_batch_size: int = 1000,
    ):
        """Initialize the scoring pipeline.

        Args:
            database_url: PostgreSQL connection string.
            engine: RankingEngine for score calculation.
            output_mode: Where to output scores (postgres, kafka, or all).
            kafka_broker: Kafka broker address (required if output_mode includes kafka).
            kafka_topic: Kafka topic for scores.
            kafka_username: SASL username for Kafka authentication.
            kafka_password: SASL password for Kafka authentication.
            kafka_ssl_ca_pem: Custom CA certificate for Kafka SSL.
            score_batch_size: Maximum scores per Kafka message batch.
        """
        self.engine = engine
        self.database_url = database_url
        self.output_mode = output_mode
        self.scoring_data: ScoringData | None = None

        # Initialize writer for postgres modes
        self._writer: ScoringDataWriter | None = None
        if output_mode in (OutputMode.POSTGRES, OutputMode.ALL):
            self._writer = ScoringDataWriter(database_url)

        # Initialize emitter for kafka modes
        self._emitter: ScoringDataEmitter | None = None
        if output_mode in (OutputMode.KAFKA, OutputMode.ALL):
            if not kafka_broker:
                raise ValueError("KAFKA_BROKER is required when OUTPUT_MODE includes kafka")
            self._emitter = ScoringDataEmitter(
                broker=kafka_broker,
                topic=kafka_topic,
                batch_size=score_batch_size,
                username=kafka_username,
                password=kafka_password,
                ssl_ca_pem=kafka_ssl_ca_pem,
            )

    def run(self) -> tuple[int, int]:
        """Run the full scoring pipeline."""
        logger.info("Starting scoring pipeline (output_mode=%s)", self.output_mode.value)

        # Create parent transaction for the entire pipeline
        with sentry_sdk.start_transaction(op="pipeline.run", name="scoring_pipeline"):
            with sentry_sdk.start_span(op="pipeline.fetch_data", description="Fetch scoring data"):
                self._fetch_data()

            with sentry_sdk.start_span(op="pipeline.rank_spaces", description="Rank spaces"):
                self._rank_spaces()

            with sentry_sdk.start_span(op="pipeline.rank_entities", description="Rank entities"):
                self._rank_entities()

            with sentry_sdk.start_span(op="pipeline.write_scores", description="Write scores"):
                self._write_scores()

        entity_count = len(self.scoring_data.entities)
        space_count = len(self.scoring_data.spaces)
        logger.info(
            "Scoring pipeline completed: %d entities, %d spaces",
            entity_count,
            space_count,
            extra={
                "pipeline_entities_processed": entity_count,
                "pipeline_spaces_processed": space_count,
            },
        )

        return entity_count, space_count

    def _fetch_data(self) -> None:
        """Fetch data from the database."""
        logger.info("Fetching scoring data from database")
        provider = ScoringDataProvider(self.database_url)
        scoring_data = provider.fetch_all()
        if scoring_data is None:
            raise RuntimeError("Scoring data not initialized")
        self.scoring_data = scoring_data

        logger.info(
            "Fetched %d entities, %d spaces, %d users, %d votes",
            len(self.scoring_data.entities),
            len(self.scoring_data.spaces),
            len(self.scoring_data.users),
            len(self.scoring_data.votes),
            extra={
                "entities_count": len(self.scoring_data.entities),
                "spaces_count": len(self.scoring_data.spaces),
                "users_count": len(self.scoring_data.users),
                "votes_count": len(self.scoring_data.votes),
            },
        )

    def _rank_spaces(self) -> None:
        """Rank spaces by their scores."""
        logger.info("Ranking spaces")
        self.scoring_data.spaces = self.engine.rank_spaces(
            self.scoring_data.spaces,
            self.scoring_data.entities,
            self.scoring_data.users,
        )

        logger.info(
            "Ranked %d spaces",
            len(self.scoring_data.spaces),
            extra={"spaces_ranked": len(self.scoring_data.spaces)},
        )

    def _rank_entities(self) -> None:
        """Rank entities by their scores."""
        logger.info("Ranking entities")
        self.scoring_data.entities = self.engine.rank_entities(
            self.scoring_data.entities,
            self.scoring_data.votes,
            self.scoring_data.users,
            self.scoring_data.spaces,
        )

        logger.info(
            "Ranked %d entities",
            len(self.scoring_data.entities),
            extra={"entities_ranked": len(self.scoring_data.entities)},
        )

    def _write_scores(self) -> None:
        """Write/emit scores based on output mode."""
        entities = self.scoring_data.entities
        spaces = self.scoring_data.spaces

        # Write to PostgreSQL if configured
        if self._writer:
            with sentry_sdk.start_span(op="pipeline.write_postgres", description="Write to PostgreSQL"):
                logger.info("Writing scores to PostgreSQL")
                self._writer.write_all(entities, spaces)

                logger.info("Scores written to PostgreSQL successfully")

        # Emit to Kafka if configured
        if self._emitter:
            with sentry_sdk.start_span(op="pipeline.emit_kafka", description="Emit to Kafka"):
                logger.info("Emitting scores to Kafka")
                self._emitter.emit_all(entities, spaces)
                remaining = self._emitter.flush()

                if remaining > 0:
                    logger.warning("Some Kafka messages may not have been delivered: %d remaining", remaining)
                else:
                    logger.info(
                        "Scores emitted to Kafka successfully: %d messages, %d errors",
                        self._emitter.messages_produced,
                        self._emitter.delivery_errors,
                        extra={
                            "kafka_messages_produced": self._emitter.messages_produced,
                            "kafka_delivery_errors": self._emitter.delivery_errors,
                        },
                    )


def main() -> None:
    """Run the scoring pipeline."""
    load_dotenv()

    # Initialize Sentry (optional, for production monitoring)
    sentry_dsn = os.environ.get("SENTRY_DSN")
    if sentry_dsn:
        sentry_sdk.init(
            dsn=sentry_dsn,
            traces_sample_rate=float(os.environ.get("SENTRY_TRACES_SAMPLE_RATE", "1.0")),
            environment=os.environ.get("SENTRY_ENVIRONMENT", "production"),
            release=os.environ.get("SENTRY_RELEASE"),
            send_default_pii=os.environ.get("SENTRY_SEND_DEFAULT_PII", "").lower() == "true",
            debug=os.environ.get("SENTRY_DEBUG", "").lower() == "true",
            integrations=[
                LoggingIntegration(
                    level=logging.INFO,  # Capture info and above as breadcrumbs
                    event_level=logging.ERROR,  # Send errors as events
                )
            ],
            enable_logs=True
        )
        print(
            f"Sentry monitoring enabled (environment: {os.environ.get('SENTRY_ENVIRONMENT', 'production')})"
        )
    else:
        print("Sentry monitoring disabled (set SENTRY_DSN to enable)")

    log_level = os.environ.get("LOG_LEVEL", "INFO").upper()
    logging.basicConfig(
        level=getattr(logging, log_level, logging.INFO),
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
        handlers=[logging.StreamHandler(sys.stdout)],
    )

    # Database configuration (required)
    database_url = os.environ.get("DATABASE_URL")
    if not database_url:
        logger.error("DATABASE_URL environment variable is required")
        sys.exit(1)

    # Output mode configuration
    output_mode_str = os.environ.get("OUTPUT_MODE", "all").lower()
    try:
        output_mode = OutputMode(output_mode_str)
    except ValueError:
        logger.error("Invalid OUTPUT_MODE: %s. Must be one of: postgres, kafka, all", output_mode_str)
        sys.exit(1)

    # Kafka configuration
    kafka_broker = os.environ.get("KAFKA_BROKER", "localhost:9092")
    environment = os.environ.get("ENVIRONMENT")
    if output_mode in (OutputMode.KAFKA, OutputMode.ALL) and not environment:
        logger.error("ENVIRONMENT variable is required when OUTPUT_MODE includes kafka")
        sys.exit(1)
    kafka_topic_base = os.environ.get("KAFKA_TOPIC", "curation.scores")
    kafka_topic = f"{environment}.{kafka_topic_base}" if environment else kafka_topic_base
    kafka_username = os.environ.get("KAFKA_USERNAME")
    kafka_password = os.environ.get("KAFKA_PASSWORD")
    kafka_ssl_ca_pem = os.environ.get("KAFKA_SSL_CA_PEM")
    score_batch_size = int(os.environ.get("SCORE_BATCH_SIZE", "1000"))

    # Ranking engine configuration
    config = RankingConfig(
        use_contestation_score=os.environ.get("USE_CONTESTATION_SCORE", "False").lower() == "true",
        use_time_decay=os.environ.get("USE_TIME_DECAY", "False").lower() == "true",
        time_decay_factor=float(os.environ.get("TIME_DECAY_FACTOR", "0.1")),
        include_subspace_votes=os.environ.get("INCLUDE_SUBSPACE_VOTES", "False").lower() == "true",
        use_activity_metrics=os.environ.get("USE_ACTIVITY_METRICS", "True").lower() == "true",
        use_distance_weighting=os.environ.get("USE_DISTANCE_WEIGHTING", "True").lower() == "true",
        distance_weight_base=float(os.environ.get("DISTANCE_WEIGHT_BASE", "0.8")),
        max_distance=int(os.environ.get("MAX_DISTANCE", "10")),
        normalize_scores=os.environ.get("NORMALIZE_SCORES", "True").lower() == "true",
        normalization_method=os.environ.get("NORMALIZATION_METHOD", "z_score"),
        filter_non_members=os.environ.get("FILTER_NON_MEMBERS", "False").lower() == "true",
        require_space_membership=os.environ.get("REQUIRE_SPACE_MEMBERSHIP", "False").lower() == "true",
    )
    engine = RankingEngine(config)

    try:
        pipeline = ScoringPipeline(
            database_url=database_url,
            engine=engine,
            output_mode=output_mode,
            kafka_broker=kafka_broker,
            kafka_topic=kafka_topic,
            kafka_username=kafka_username,
            kafka_password=kafka_password,
            kafka_ssl_ca_pem=kafka_ssl_ca_pem,
            score_batch_size=score_batch_size,
        )
        entity_count, space_count = pipeline.run()

        # Add success context to Sentry
        sentry_sdk.set_tag("pipeline.status", "success")
        sentry_sdk.set_tag("output_mode", output_mode.value)
        sentry_sdk.set_context(
            "pipeline.results",
            {
                "entity_count": entity_count,
                "space_count": space_count,
            },
        )
    except Exception:
        # Add error context to Sentry
        sentry_sdk.set_tag("pipeline.status", "error")
        sentry_sdk.set_tag("output_mode", output_mode.value)
        logger.exception("Scoring pipeline failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
