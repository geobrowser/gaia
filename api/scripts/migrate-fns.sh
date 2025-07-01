#!/bin/bash

source .env

echo "Running SQL scripts..."
psql $DATABASE_URL < drizzle/0002_unusual_shen.sql
echo "Functions successfully added."
