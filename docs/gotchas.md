Quick list of gotchas, quirks and edge cases to be aware of when working in Gaia + Hermes

### IPFS cache timing + error handling

IPFS network latency is by far the largest bottleneck in our system. To alleviate this, we have a system for concurrently prefetching IPFS contents. The cache listens for edit contents from published edits and proposed edits, reads the hash from IPFS, then writes the data to a cache.

The Hermes Pipeline reads from the cache whenever it needs to populate Kafka events with edit data. The pipeline implements jittered, exponential retries with backoff in cases where the cache is not reachable or the cache hasn't yet populated the edit contents.

Hermes Pipeline's retry is bounded, after which it will skip the edit event with a error log/trace.

### Edit size limits

Geo does not limit edit sizes at the protocol level. Instead we institute limits in Gaia + Hermes. Currently the limit in Kafka is 20MB, although we should lower it to 10MB in accordance with recent alignment (Feb 2026). We should fail open and log edits that exceed the limit instead of crashing the pipeline service entirely.

Note that this 20MB limit is set in code, but also in our Kafka instance. The limit is both globally configured at the Kafka level, but also at the topic level.

### Kafka event ordering

The Geo protocol emits ordered events from our blockchain. Events often have dependencies on previous events, which means that ordering is quite important. Since we are reifying blockchain events into enriched Kafka events, we need to make sure our Kafka events adhere to the same ordering. Right now we have a few mechanisms to preserve ordering. We also serially index events block-by-block instead of parallelizing ingestion.

Consumers also care about the ordering, and often want to ingest the contents of a block fully before indexing the next block. Since Kafka doesn't have blocks, we add enriched metadata to each event including block information as well as metadata describing whether a block has been fully emitted to Kafka.

1. There is a `is_last` flag on the last event emitted in a block
2. There is a `BlockEnded` event to denote there are no more events coming from Kafka for that block

This works but is not entirely ideal. Consumers could miss the `is_last` field if they're not indexing the event which has the flag. Consumers also would need to ingest the `BlockEnded` event since it's a bit safer.

There's also no timing guarantees from Kafka, so if a consumer is waiting for a block checkpoint there's no guarantee as to WHEN they will receive it. kg-indexer solves this by buffering events in a time window. If the checkpoint event does not arrive in the window it flushes its buffered data to its database.

In the future we could consolidate block checkpoints into a single mechanism.

### Pipeline persistence

Hermes Pipeline does not persist its indexed state. This means that redeployments or restarts will run the entire pipeline from scratch and re-emit events. We should consider adding persistence if this becomes an issue.

### Consumer idempotency

Consumers should be idempotent to to handle cases where producers re-emit events.

### Staging and Production environments

We have dual environments for the Gaia + Hermes stack. Currently both production and staging are hosted on our k8s cluster and paritioned using namespaces. For example, `knowledge` vs `knowledge-staging`.

### Multiple producers

Gaia + Hermes have two producers: Atlas and Hermes Pipeline. They emit different events so we currently don't worry about ordering or conflicts. If we add multiple producers in the future we'll need to be care about how we handle event ordering for downstream consumers.

### Kafka emission guarantees

Right now we use a FutureProducer in Hermes Pipeline to await a response from Kafka when emitting an event so we know there's some guarantee of delivery. Previously we used BaseProducer and there could be situations where we sent an event to Kafka but it failed to deliver without any errors. It hasn't been benchmarked, but FutureProducer probably slows our throughput quite considerably. Probably doesn't matter in practice except in situations where we index from the beginning.

### Indexer performance, hot path sidecars, and custom databases

In kg-indexer we try and optimize write throughput by avoiding any reads in the hot path. There are some situations where we NEED to read data before writing. For those situations we currently have async sidecar indexers that operate within the same kg-indexer process.

One example of this is the proposal vote tally. We currently don't emit the total YES/NO vote counts from the protocol as users vote on proposals. Knowing the vote counts are useful for UI display, but also useful to compute the status of a proposal, like whether it's executable or not, without having to read from the chain RPC.

For this we have a proposal vote tally sidecar that runs in kg-indexer but does not block the main indexing thread. This does introduce some durability/persistence problems to solve, but it works at the current scale.

Generally we should try and make the main kg-indexer hot path as performant as possible since we only have one global indexer. Future indexer sharding implementations can help here as well.

We currently have quite a few indexes due to the nature of our existing query patterns. We can probably audit these, but my guess is that most of them need to stick around. This could be alleviated if we move away from GraphQL and user-driven querying. These indexes impact our write throughput quite a bit in Postgres.

For very exploratory future work, we can look at implementing a custom database oriented specifically to our needs. This DB can run in memory and ocassionally flush checkpoints to disk. Since we have the blockchain, IPFS, and Kafka in front of the indexer, we can actually get away with not having to implement a lot of the durability mechanisms that traditional databases do. Additionally, our data model is very well-scoped, so we can build the right indexes and storage mechanics specifically for our needs right into the database. We can implement a disaggregated storage database which a lot of modern databases are moving to.
