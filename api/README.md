## Restoring the local database from the staging database

```sh
# Dump the staging database
/usr/local/opt/postgresql@16/bin/pg_dump "$DATABASE_URL" \
  --format=custom \
  --no-owner \
  --no-privileges \
  --verbose \
  --file testnet-postgres.dump

# Copy the dump to the postgres container
docker cp ./testnet-postgres.dump $(docker compose ps -q postgres):/testnet-postgres.dump

# Drop the local database
docker compose exec -T postgres psql -U postgres -d gaia -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"

# Restore the dump to the local database
docker compose exec -T postgres pg_restore \
  -U postgres \
  -d gaia \
  --no-owner \
  --role=postgres \
  --verbose \
  /testnet-postgres.dump
```