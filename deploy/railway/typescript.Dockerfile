# syntax=docker/dockerfile:1.7
FROM node:24-bookworm-slim AS runtime
RUN useradd --create-home --uid 10001 keeper
WORKDIR /app
COPY --chown=keeper:keeper package.json protocol.lock.json ./
COPY --chown=keeper:keeper typescript ./typescript
USER keeper
ENV DUSK_PROTOCOL_LOCK=/app/protocol.lock.json
ENV KEEPER_MODE=shadow
EXPOSE 8080
ENTRYPOINT ["node", "--experimental-strip-types", "typescript/apps/keeper/src/index.ts"]

