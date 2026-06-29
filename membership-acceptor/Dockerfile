FROM oven/bun:1.3.9 AS builder
WORKDIR /app
COPY package.json bun.lock* ./
RUN bun install --frozen-lockfile
COPY . .

FROM oven/bun:1.3.9
ENV NODE_ENV=production
RUN groupadd --system --gid 1001 geo && useradd --system --uid 1001 --gid geo geo
WORKDIR /app
COPY --from=builder --chown=geo:geo /app/package.json /app/bun.lock* ./
RUN bun install --frozen-lockfile --production
COPY --from=builder --chown=geo:geo /app/src ./src
USER geo
EXPOSE 8080
CMD ["bun", "run", "src/index.ts"]
