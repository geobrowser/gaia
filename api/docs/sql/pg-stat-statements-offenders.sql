-- Top DB offenders by total and tail latency
-- Run in the target application database.

-- 1) Highest total DB time
SELECT
  queryid,
  calls,
  round(total_exec_time::numeric, 0) AS total_ms,
  round(mean_exec_time::numeric, 2) AS mean_ms,
  round(max_exec_time::numeric, 2) AS max_ms,
  left(regexp_replace(query, '\\s+', ' ', 'g'), 180) AS query
FROM pg_stat_statements
ORDER BY total_exec_time DESC
LIMIT 25;

-- 2) Tail outliers with meaningful volume
SELECT
  queryid,
  calls,
  round(mean_exec_time::numeric, 2) AS mean_ms,
  round(max_exec_time::numeric, 2) AS max_ms,
  round(total_exec_time::numeric, 0) AS total_ms,
  left(regexp_replace(query, '\\s+', ' ', 'g'), 180) AS query
FROM pg_stat_statements
WHERE calls >= 50
ORDER BY max_exec_time DESC
LIMIT 25;

-- 3) Current lock waits (live view)
SELECT
  pid,
  now() - query_start AS age,
  wait_event_type,
  wait_event,
  state,
  left(regexp_replace(query, '\\s+', ' ', 'g'), 180) AS query
FROM pg_stat_activity
WHERE datname = current_database()
  AND state = 'active'
  AND wait_event_type = 'Lock'
ORDER BY age DESC
LIMIT 25;
