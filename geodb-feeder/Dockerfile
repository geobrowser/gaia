FROM node:22-bookworm-slim AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY tsconfig.json tsconfig.build.json ./
COPY src ./src
RUN npm run build && npm prune --omit=dev

FROM node:22-bookworm-slim
WORKDIR /app
ENV NODE_ENV=production
COPY --from=build --chown=1001:1001 /app/node_modules ./node_modules
COPY --from=build --chown=1001:1001 /app/dist ./dist
COPY --chown=1001:1001 package.json ./
# geodb-client is not in this image: the pod's init container copies it from the geodb image
# (see geodb/k8s/v2/geodb.yaml) and GEODB_CLIENT_BIN points at the copy.
USER 1001
EXPOSE 9090
CMD ["node", "dist/main.js"]
