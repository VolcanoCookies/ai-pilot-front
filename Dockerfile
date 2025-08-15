FROM rust:1.89 AS builder
WORKDIR /app

COPY . .
RUN cargo build --release

# ---
FROM rust:1.89.0-slim-trixie
WORKDIR /app

COPY --from=builder /app/target/release/aip-front /app
COPY ./templates /app/templates
COPY ./public /app/public
COPY ./Rocket.toml /app/Rocket.toml

LABEL org.opencontainers.image.source https://github.com/VolcanoCookies/ai-pilot-front

CMD ["/app/aip-front"]