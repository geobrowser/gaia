#!/bin/bash

source .env

echo "Running SQL scripts..."
psql $DATABASE_URL < drizzle/0002_brief_thanos.sql
psql $DATABASE_URL < drizzle/0003_fine_nightcrawler.sql
echo "Functions successfully added."
