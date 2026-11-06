#!/usr/bin/env bash
# Export app_webhooks (id, app_name, url, secret, notification_types, space_ids) from
# Postgres into the k8s Secret `app-webhooks` (key `webhooks.json`) that
# notification-indexer and delivery-worker read via APP_WEBHOOKS_FILE.
#
# The rows go straight from psql into `kubectl create secret ... --dry-run=client -o yaml
# | kubectl apply -f -`. Nothing is written to disk and nothing is printed: no row values,
# no secret values, not even a count. Run it yourself; re-run it after any webhook change.
#
#   DATABASE_URL=postgres://... ./export-app-webhooks.sh        # needs psql + kubectl context
#   NAMESPACE=gaia SECRET_NAME=app-webhooks               # optional overrides
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL to the v2 KG database (the one api-secrets points at)}"
NAMESPACE="${NAMESPACE:-gaia}"
SECRET_NAME="${SECRET_NAME:-app-webhooks}"

psql --no-psqlrc -X -q -At "$DATABASE_URL" -c "
  SELECT coalesce(json_agg(json_build_object(
           'id', id,
           'app_name', app_name,
           'url', url,
           'secret', secret,
           'notification_types', notification_types,
           'space_ids', space_ids
         ) ORDER BY id), '[]'::json)
  FROM app_webhooks" \
  | kubectl -n "$NAMESPACE" create secret generic "$SECRET_NAME" \
      --from-file=webhooks.json=/dev/stdin --dry-run=client -o yaml \
  | kubectl apply -f - >/dev/null

echo "applied secret/$SECRET_NAME in namespace $NAMESPACE (values not shown)"
