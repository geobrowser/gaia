# geodb-serve + geodb-client, built from geo-db `final` at a pinned commit by
# .github/workflows/build-geodb-image.yml, which puts the two static binaries next to this file.
# Local build: copy geodb-serve and geodb-client into this directory first.
FROM debian:bookworm-slim

# The supervisor needs sh, sed, tail, sleep and head (in the base image) and curl, to publish serve's
# node and sealer ids to the geodb-space-map ConfigMap. The api's init container needs getent and cut.
USER root
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY geodb-serve geodb-client /usr/local/bin/
COPY supervisor.sh /usr/local/bin/geodb-supervisor
RUN chmod 0755 /usr/local/bin/geodb-serve /usr/local/bin/geodb-client /usr/local/bin/geodb-supervisor

USER 1001
CMD ["/usr/local/bin/geodb-supervisor"]
