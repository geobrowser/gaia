FROM oven/bun:1 AS base
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
RUN addgroup --system --gid 1001 geo && \
    adduser --system --uid 1001 --ingroup geo geo

# Copy dependencies
COPY --from=deps --chown=geo:geo /app/node_modules ./node_modules

# Copy application files
COPY --chown=geo:geo src ./src
COPY --chown=geo:geo drizzle ./drizzle
COPY --chown=geo:geo package.json main.ts drizzle.config.ts tsconfig.json ./

USER geo

EXPOSE 3000

CMD ["bun", "run", "main.ts"]
