#!/bin/bash

source .env

echo "Running SQL scripts..."
psql $DATABASE_URL < drizzle/custom_0002_brief_thanos.sql
psql $DATABASE_URL < drizzle/custom_0003_fine_nightcrawler.sql
echo "Functions successfully added."
