#!/bin/bash

source .env

echo "Running SQL scripts..."
psql $DATABASE_URL < drizzle/0002_brief_thanos.sql
echo "Functions successfully added."
