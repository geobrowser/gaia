FROM oven/bun:1.3.9 AS base
WORKDIR /app

# Install dependencies
FROM base AS deps
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

# Bundle application code (TS -> JS) targeting Node
FROM base AS build
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY src ./src
COPY main.ts tsconfig.json ./
RUN bun build main.ts --target=node --outdir=dist --sourcemap=linked --format=esm

# Production image — Node runtime with bun available for migrations
FROM node:24-slim AS runner
WORKDIR /app

# Copy bun binary for migrations (drizzle-kit runs via bun)
COPY --from=base /usr/local/bin/bun /usr/local/bin/bun

ENV NODE_ENV=production

# Create non-root user
RUN groupadd --system --gid 1001 geo && \
    useradd --system --uid 1001 --gid geo geo

# Copy dependencies
COPY --from=deps --chown=geo:geo /app/node_modules ./node_modules

# Copy bundled application
COPY --from=build --chown=geo:geo /app/dist ./dist

# Copy files needed for migrations (drizzle-kit needs these)
COPY --chown=geo:geo drizzle ./drizzle
COPY --chown=geo:geo package.json drizzle.config.ts tsconfig.json ./
COPY --chown=geo:geo src/services/storage/schema.ts ./src/services/storage/schema.ts

USER geo

EXPOSE 3000

CMD ["node", "--expose-gc", "dist/main.js"]
