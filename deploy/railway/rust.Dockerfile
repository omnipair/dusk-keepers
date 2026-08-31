# syntax=docker/dockerfile:1.7
FROM rust:1.97-slim-bookworm AS builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY protocol.lock.json ./protocol.lock.json
COPY rust ./rust
RUN cargo build --locked --release --package dusk-keeper

FROM debian:bookworm-slim AS runtime
RUN useradd --create-home --uid 10001 keeper
COPY --from=builder /workspace/target/release/dusk-keeper /usr/local/bin/dusk-keeper
COPY --chown=keeper:keeper protocol.lock.json /app/protocol.lock.json
# Every profile past the sentinel reads these at startup: the instruction
# contract to encode against, the resolution manifest to derive accounts from,
# and the generated layout to decode by. Without them a keeper starts,
# discovers, and silently never acts — which is why their absence is logged
# rather than shrugged off.
COPY --chown=keeper:keeper protocol/keeper-instructions.v1.json /app/protocol/keeper-instructions.v1.json
COPY --chown=keeper:keeper protocol/keeper-account-resolution.v1.json /app/protocol/keeper-account-resolution.v1.json
COPY --chown=keeper:keeper protocol/keeper-account-layout.v1.json /app/protocol/keeper-account-layout.v1.json
USER keeper
WORKDIR /app
ENV DUSK_PROTOCOL_LOCK=/app/protocol.lock.json
ENV KEEPER_MODE=shadow
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/dusk-keeper"]

