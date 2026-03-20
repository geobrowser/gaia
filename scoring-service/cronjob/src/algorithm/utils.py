"""Utility functions for scoring algorithm."""

import logging
import time

from .models import Space

logger = logging.getLogger(__name__)


def calculate_space_distances(
    spaces: list[Space], max_distance: int = 10
) -> dict[tuple[str, str], int]:
    """Calculate distances between all pairs of spaces using BFS.

    Returns a hashtable where key is (space_id_1, space_id_2) and value is distance.
    Distance is symmetric, so both (A, B) and (B, A) are included.
    If no path exists or distance > max_distance, the pair is not included.
    """
    # Build adjacency list for space hierarchy
    adjacency: dict[str, list[str]] = {}
    for space in spaces:
        if space.id not in adjacency:
            adjacency[space.id] = []

        # Add bidirectional parent-child edges
        if space.parent_space_id:
            if space.parent_space_id not in adjacency:
                adjacency[space.parent_space_id] = []

            adjacency[space.parent_space_id].append(space.id)
            adjacency[space.id].append(space.parent_space_id)

    logger.info("Calculating space distances (%d spaces, max_distance=%d)", len(spaces), max_distance)
    t0 = time.monotonic()

    # Calculate distances using BFS from each space
    distances: dict[tuple[str, str], int] = {}

    for start_space in spaces:
        # BFS to find distances from start_space to all other spaces
        queue = [(start_space.id, 0)]
        visited = {start_space.id}

        while queue:
            current_space_id, current_distance = queue.pop(0)

            distances[(start_space.id, current_space_id)] = current_distance

            # Stop if maximum distance reached
            if current_distance >= max_distance:
                continue

            # Explore neighbouring spaces
            if current_space_id in adjacency:
                for neighbor_id in adjacency[current_space_id]:
                    if neighbor_id not in visited:
                        visited.add(neighbor_id)
                        queue.append((neighbor_id, current_distance + 1))

    logger.info("Space distances calculated in %.1fs (%d pairs)", time.monotonic() - t0, len(distances))
    return distances
