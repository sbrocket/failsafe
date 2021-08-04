ARG RUST_VERSION=1.53.0-alpine

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

FROM alpine:3.14 as runtime
COPY --from=builder /app/target/release/failsafe /usr/local/bin
# These UIDs and GIDs match the host system, configured through cloud-init, to ensure that bind
# mount permissions behave as expected.
RUN addgroup -g 61000 -S failsafe && adduser -g 61000 -S failsafe -G failsafe \
    && addgroup -g 62000 -S storage_grp && addgroup failsafe storage_grp
USER failsafe
ENTRYPOINT ["/usr/local/bin/failsafe"]