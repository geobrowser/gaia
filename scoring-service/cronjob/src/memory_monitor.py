"""Background memory monitor that alerts via Sentry when usage exceeds a threshold."""

import logging
import threading

import psutil
import sentry_sdk

logger = logging.getLogger(__name__)


def _read_cgroup_memory_limit() -> int | None:
    """Read the container memory limit from cgroup filesystem.

    Returns:
        Memory limit in bytes, or None if not running in a container.
    """
    # cgroup v2
    try:
        value = open("/sys/fs/cgroup/memory.max").read().strip()
        if value != "max":
            return int(value)
    except (FileNotFoundError, ValueError):
        pass

    # cgroup v1
    try:
        value = int(open("/sys/fs/cgroup/memory/memory.limit_in_bytes").read().strip())
        # cgroup v1 returns a very large number when unlimited
        if value < 2**62:
            return value
    except (FileNotFoundError, ValueError):
        pass

    return None


def start_memory_monitor(threshold: float = 0.85, interval: float = 5.0) -> None:
    """Start a daemon thread that monitors memory usage.

    When RSS exceeds the threshold percentage of the container memory limit,
    logs a warning and sends a Sentry alert. Only alerts once per crossing
    to avoid spam.

    Args:
        threshold: Memory usage ratio (0.0-1.0) to trigger alert.
        interval: Seconds between checks.
    """
    limit = _read_cgroup_memory_limit()
    if limit is None:
        logger.info("Memory monitor: no cgroup limit detected, skipping")
        return

    logger.info(
        "Memory monitor: started (limit=%dMB, threshold=%.0f%%, interval=%.0fs)",
        limit // 1024 // 1024,
        threshold * 100,
        interval,
    )

    def _monitor() -> None:
        process = psutil.Process()
        alerted = False

        while True:
            rss = process.memory_info().rss
            usage = rss / limit

            if usage > threshold and not alerted:
                msg = (
                    f"Memory usage critical: {usage:.0%} "
                    f"({rss // 1024 // 1024}MB / {limit // 1024 // 1024}MB)"
                )
                logger.warning(msg)
                sentry_sdk.capture_message(msg, level="warning")
                alerted = True
            elif usage <= threshold:
                alerted = False

            threading.Event().wait(interval)

    t = threading.Thread(target=_monitor, daemon=True)
    t.start()
