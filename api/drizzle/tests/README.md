# SQL-level migration tests

Assertions for migrations whose logic lives in SQL functions, where the behaviour
is easier to pin at the SQL layer than through the app.

**These now run in CI.** `api/src/kg/__tests__/rankingSqlSuites.test.ts` executes every
`.sql` file in this directory as part of the API Integration Tests job, in forward AND
reverse order, against a scratch database it creates and drops per run. Adding a file
here is enough to get it run; it must be named after the migration it covers. A migration
that has no suite of its own but is still needed to build the schema goes in that file's
`EXTRA_MIGRATIONS`.

Run them by hand against a throwaway Postgres the same way (never a real database — these
truncate tables):

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

Three ways these have gone stale, all worth guarding against:

* 0075 added the name requirement, which invalidated 0074's fixtures — none of them
  had a name, so every 0074 assertion passed vacuously except the one counting rows,
  which failed. A migration that narrows candidate generation invalidates every
  earlier suite's fixtures; fix by making the fixtures satisfy the new rule, not by
  adjusting the expected count.
* An assertion can go green because the thing it names is unreachable. Mutating the
  code it claims to cover is the only reliable check.
* 0083 made participation count curation votes alongside stance. 0078's entity 1 carries
  a curation downvote, so it went from 5 votes to 6 and its direction-agnostic assertion
  — that a 3/2 split scores the same as 5/0 — started comparing 6 votes against 5. It had
  been failing for days when the CI harness was written, and nobody could have known.
  Fixed by giving it a partner with the same composition (entity 5), not by relaxing the
  assertion: the property was still true, only the fixture had gone stale.

Known limitation: these run against `0073_fixtures_schema.sql`, which is hand-maintained
and can drift from the migrations it stands in for. They cannot run against the real
schema because their `TRUNCATE`s fail there — `spaces` and `subspace_topics` now carry
foreign keys to `entities`, and the closure pulls in `proposals`, `proposal_votes`,
`subspaces` and `space_voting_settings`. 0089 and 0090 avoid this by deleting only their
own rows by id; converting the older files the same way is what would let the whole set
run against the real schema, and is the next thing worth doing here.
