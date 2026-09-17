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
truncate tables, now with CASCADE):

```bash
docker run -d --name migtest -p 55433:5432 -e POSTGRES_PASSWORD=t -e POSTGRES_USER=t -e POSTGRES_DB=t postgres:16
cd ../..  # api/
DATABASE_URL_DIRECT=postgresql://t:t@localhost:55433/t bun drizzle-kit migrate
docker cp drizzle/tests/0078_ranking_participation.sql migtest:/tmp/s.sql
docker exec migtest psql -U t -d t -v ON_ERROR_STOP=1 -f /tmp/s.sql
docker rm -f migtest
```

**The whole real schema, applied by the real migrator.** These used to run against a
hand-written `0073_fixtures_schema.sql` that recreated a few columns of four tables. It
had drifted, in the direction that hides bugs: it declared every `entities` column except
`id` nullable, where production has `created_at_block`, `updated_at` and `updated_at_block`
NOT NULL — so every suite inserted rows the real database would have rejected. It is gone.

Suites pass in any order — verified by running the set forwards and backwards, which the
CI harness does on every run. 0073 originally had no `TRUNCATE` and only passed when it
ran first against a virgin database; its row-count assertions counted every row in
`entity_ranking_scores`, so a fixture left behind by any other suite failed it.

Several suites assert absolute counts over the whole feed (`entities_ranked_for_feed()`),
so they need an EMPTY database, not merely a clean set of their own rows. The scratch
database is empty by construction; do not point these at a database with content.

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

In CI each suite runs inside a transaction that is **rolled back**, which is what isolates
them from each other; their own `TRUNCATE`/`DELETE` cleanup is for running them by hand
through psql, where there is no such wrapper. The `TRUNCATE`s need `CASCADE` against the
real schema: `spaces` and `subspace_topics` carry foreign keys to `entities`, and the
closure pulls in `proposals`, `proposal_votes`, `subspaces` and `space_voting_settings`.
That is a wide blast radius by design — another reason never to point these at a database
with content in it.

`assert(cond, label)` is defined per-file and uses **`IS NOT TRUE`**, not `NOT cond`. The
difference is not pedantic: under `NOT cond` a **NULL** condition is neither true nor false,
so it takes the ELSE branch and reports a **pass**. A `SELECT ... INTO` matching no row, or
a scalar subquery matching no row, therefore passes vacuously — the trap listed above, and
one that a draft of 0091 fell into. Keep the strict form in any new suite.

Converting the older suites to it surfaced no vacuous assertions; that change is
preventive, not a repair.
