# We have to cross compile the musl linked binary on Debian due to Rust proc macros
# requiring dynamic library support which isn't available on the Alpine platform.
FROM rust:1.41-buster as builder

WORKDIR /usr/src/pangolin

RUN rustup target add x86_64-unknown-linux-musl \
  && apt-get update \
  && apt-get install -y \
     build-essential \
     musl-dev \
     musl-tools

COPY Cargo* ./
COPY src/ ./src/

RUN cargo install --target x86_64-unknown-linux-musl --path .

FROM alpine:3.11.3

COPY --from=builder /usr/local/cargo/bin/pangolin /usr/local/bin/pangolin

CMD ["sh", "-c", "pangolin"]