## Running the data service stack

The knowledge graph data service is comprised of three components: 1) Indexers, 2) an IPFS cache, and 3) the API. The indexers read through the knowledge graph blockchain serially and index relevant events sequentially. For any events that read from IPFS, it reads from the IPFS cache. Reading from IPFS can be slow, especially for large files, so the IPFS cache is a separate process that reads through the chain in an optimized way and writes the IPFS contents to a local store on disk. Lastly the API reads indexed data from the database and serves it to consumers in an ergnomic way.

### Install dependencies and run migrations

The data service is dependent on the following tools:

- [Rust](https://www.rust-lang.org/)
- [PostgreSQL](https://www.postgresql.org/)
- [Bun](https://bun.sh/)

The database has an expected schema for the IPFS cache and indexers. For now all of the schemas are managed through the API project.

To run migrations, first populate a `.env` file in the `/api` directory with the following:

```sh
DATABASE_URL=""
```

Then run the following commands from within the `/api` directory:

```sh
bun install
bun drizzle-kit migrate
```

If done correctly you should see logs signaling a successful migration.

### Running the IPFS cache

The indexers depend on the IPFS cache to handle preprocessing of IPFS contents. To run the cache populate an `.env` file in the root of this directory.

```sh
SUBSTREAMS_API_TOKEN=""
SUBSTREAMS_ENDPOINT=""
DATABASE_URL=""
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

### Other indexers

Currently only the knowledge graph indexer is implemented, but in the near future there will be other indexers for processing governance events or managing the knowledge graph's history.
