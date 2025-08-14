FROM rust:1.89 AS builder
WORKDIR /app

COPY . .
RUN cargo build --release

# ---
FROM rust:1.89.0-alpine3.22
WORKDIR /app

COPY --from=builder /app/target/release/aip-front /app
COPY ./templates /app/templates
COPY ./public /app/public
COPY ./Rocket.toml /app/Rocket.toml

CMD ["aip-front"]