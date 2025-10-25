FROM rust:1.90-slim as builder
WORKDIR /src
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml /src/Cargo.toml
COPY operator/Cargo.toml /src/operator/Cargo.toml
# Create a dummy src to cache deps
#RUN mkdir -p /src/operator/src && echo 'fn main(){}' > /src/operator/src/main.rs
# RUN cargo build --release -p operator || true
# Now copy real sources
COPY operator /src/operator
RUN cargo build --release -p operator

# Runtime stage
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /src/target/release/operator /usr/local/bin/operator
ENTRYPOINT ["/usr/local/bin/operator"]
