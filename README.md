## Running the data service stack

The knowledge graph data service is comprised of three components: 1) Indexers, 2) an IPFS cache, and 3) the API. The indexers read through the knowledge graph blockchain serially and index relevant events sequentially. For any events that read from IPFS, it reads from the IPFS cache. Reading from IPFS can be slow, especially for large files, so the IPFS cache is a separate process that reads through the chain in an optimized way and writes the IPFS contents to a local store on disk. Lastly the API reads indexed data from the database and serves it to consumers in an ergonomic way.

### Install dependencies and run migrations

The data service is dependent on the following tools:

- [Rust](https://www.rust-lang.org/)
- [PostgreSQL](https://www.postgresql.org/)
- [Bun](https://bun.sh/)

The database has an expected schema for the IPFS cache and indexers. For now all of the schemas are managed through the API project.

To run migrations, first populate an `.env` file in the `/api` directory with the following:

```sh
DATABASE_URL="postgresql://localhost:5432/gaia" # or any connection string
CHAIN_ID="80451" # or 19411 for mainnet
IPFS_KEY=''
IPFS_GATEWAY_WRITE=''
IPFS_GATEWAY_READ=''
IPFS_ALTERNATIVE_GATEWAY_KEY='' # when using Pinata according to the docs example use JWT (it contains the API secret)
IPFS_ALTERNATIVE_GATEWAY_WRITE=''
RPC_ENDPOINT=''
DEPLOYER_PK=''
```

You can run a PostgreSQL container using the `docker compose up` command and then set the `DATABASE_URL` to `postgresql://postgres:postgres@localhost:5432/gaia`.

Then run the following commands from within the `/api` directory:

```sh
bun install
bun run db:migrate
```

If done correctly, you should see logs signaling a successful migration.

### Running the IPFS cache

The indexers depend on the IPFS cache to handle preprocessing of IPFS contents. To run the cache, populate an `.env` file in the root of this directory.

```sh
SUBSTREAMS_API_TOKEN=""
SUBSTREAMS_ENDPOINT=""
DATABASE_URL="postgresql://localhost:5432/gaia" # or any connection string
```

Then run the following command

```sh
cargo run -p cache
# or with the --release flag to run in "production" mode
# cargo run -p cache --release
```

If done correctly you should see the indexer begin processing events and writing data to the `ipfs_cache` table in your postgres database.

The cache will continue to populate so long as the Rust process is still executing. If you run the process again, it will start from the beginning of the chain, but skip any cache entries that already exist in the database.

### Running the knowledge graph indexer

The knowledge graph indexer reads through the chain sequentially, listening for any events related to published edits. When it encounters an IPFS hash it reads from the cache, runs any transformations, then writes to the database.

To run the knowledge graph indexer, run the following commands:

```sh
cargo run -p indexer
# or with the --release flag to run in "production" mode
# cargo run -p indexer --release
```

If done correctly you should see the indexer begin processing the knowledge graph events sequentially.

### Running the actions indexer

The actions indexer processes all knowledge graph onchain actions. Currently the only action implemented is entity curation/voting.

To run the actions indexer, run the following commands:

```sh
cargo run -p actions-indexer
# or with the --release flag to run in "production" mode
# cargo run -p indexer --release
```

### Other indexers

Currently only the knowledge graph indexer is implemented, but in the near future there will be other indexers for processing governance events or managing the knowledge graph's history.

## Documentation

Architecture and design documents are in the `docs/` directory:

- [Hermes Architecture](docs/hermes-architecture.md) - Event streaming from blockchain to Kafka
- [K8s Secrets Isolation](docs/k8s-secrets-isolation.md) - Kubernetes secrets management

Project-specific documentation lives in each project's directory:
- [Atlas](atlas/docs/) - Canonical graph computation
- [Hermes Substream](hermes-substream/docs/) - Event filtering and modification
