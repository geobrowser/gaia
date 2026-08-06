FROM oven/bun:1.3.9 AS base
WORKDIR /app

# Install dependencies
FROM base AS deps
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

# Production image
FROM base AS runner
WORKDIR /app

ENV NODE_ENV=production

# Create non-root user
RUN groupadd --system --gid 1001 geo && \
    useradd --system --uid 1001 --gid geo geo

# Copy dependencies
COPY --from=deps --chown=geo:geo /app/node_modules ./node_modules

# Copy application files
COPY --chown=geo:geo src ./src
COPY --chown=geo:geo drizzle ./drizzle
COPY --chown=geo:geo package.json main.ts drizzle.config.ts tsconfig.json ./

USER geo

EXPOSE 3000

# --smol makes JSC collect far more eagerly instead of growing the heap
# opportunistically. Bun does not reliably size its heap against the cgroup
# limit, so in a 2Gi container (limits.memory in api/k8s/*/api.yaml) it can
# walk past the limit and be OOMKilled before GC gets serious. Costs some
# throughput; cheaper than a restart loop.
CMD ["bun", "--smol", "run", "main.ts"]
