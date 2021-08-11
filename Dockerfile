ARG RUST_VERSION=latest

FROM rust:${RUST_VERSION} as base
WORKDIR /app
# Install musl-dev on Alpine to avoid error "ld: cannot find crti.o: No such file or directory"
RUN ((cat /etc/os-release | grep ID | grep alpine) && apk add --no-cache musl-dev || true) \
    && cargo install cargo-chef \
    && rm -rf $CARGO_HOME/registry/

FROM base as planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM base as cacher
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

FROM base as builder
WORKDIR /app
COPY . .
# Copy over the cached dependencies
COPY --from=cacher /app/target target
COPY --from=cacher $CARGO_HOME $CARGO_HOME
RUN cargo build --release --bin failsafe

FROM ubuntu:20.04 as runtime
COPY --from=builder /app/target/release/failsafe /usr/local/bin
# dumb-init is used so that failsafe can handle signals normally, instead of getting the special PID
# 1 behavior
RUN apt-get update && apt-get install -y dumb-init && rm -rf /var/lib/apt/lists/*
# Within the container, run as an unprivileged user with a fixed uid. The fixed uid is used by the
# host system to set up correct permissions for mapped volumes.
RUN adduser --system --uid 128 --group failsafe-bot
USER 128:128
ENTRYPOINT ["/usr/bin/dumb-init", "/usr/local/bin/failsafe"]
