# SQL-level migration tests

Assertions for migrations whose logic lives in SQL functions, where the behaviour
is easier to pin at the SQL layer than through the app.

Run against a throwaway Postgres (never a real database — these truncate tables):

```bash
docker run -d --name migtest -e POSTGRES_PASSWORD=t -e POSTGRES_DB=t postgres:18
docker cp 0073_fixtures_schema.sql migtest:/tmp/ && docker exec migtest psql -U postgres -d t -f /tmp/0073_fixtures_schema.sql
sed 's/^--> statement-breakpoint$//' ../0073_entity_ranking_scores.sql > /tmp/m.sql
docker cp /tmp/m.sql migtest:/tmp/ && docker exec migtest psql -U postgres -d t -v ON_ERROR_STOP=1 -f /tmp/m.sql
docker cp 0073_entity_ranking_scores.sql migtest:/tmp/ && docker exec migtest psql -U postgres -d t -v ON_ERROR_STOP=1 -f /tmp/0073_entity_ranking_scores.sql
docker rm -f migtest
```

`0073_fixtures_schema.sql` recreates only the columns of `entities`, `values`,
`relations` and `votes_count` that the scoring functions read, verified against the
live schema. It is a test fixture, not a source of truth for those tables.

Apply migrations 0073–0077 in order, then run the suites. They truncate their own
fixtures, so they pass in any order — verified by running the set forwards and
backwards. 0073 originally had no `TRUNCATE` and only passed when it ran first
against a virgin database; its row-count assertions counted every row in
`entity_ranking_scores`, so a fixture left behind by any other suite failed it.

These were mutation-tested: flipping the recency sign, replacing Wilson with
`abs(net)`, dropping the `is_system` filter, and dropping the `vote_kind = 0` filter
each fail exactly one assertion. For 0077, dropping the `space_ids` predicate,
neutering `type_ids`, negating it (`= ANY` → `<> ALL`), and dropping the global type
exclusion are each caught. Worth re-checking if you change the assertions —
the `is_system` case originally passed a mutation because the assertion that claimed
to cover it was written against an entity with no relations at all.

Two ways these have gone stale before, both worth guarding against:

* 0075 added the name requirement, which invalidated 0074's fixtures — none of them
  had a name, so every 0074 assertion passed vacuously except the one counting rows,
  which failed. A migration that narrows candidate generation invalidates every
  earlier suite's fixtures; fix by making the fixtures satisfy the new rule, not by
  adjusting the expected count.
* An assertion can go green because the thing it names is unreachable. Mutating the
  code it claims to cover is the only reliable check.

TODO: port to the vitest harness in `api/src/kg/__tests__` so they run in CI. Until
then nothing here runs automatically, which is how both staleness cases above
survived.
