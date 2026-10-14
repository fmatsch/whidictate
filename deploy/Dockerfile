# Server-Image: docker build -f deploy/Dockerfile -t whidictate-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p whidictate-server

FROM debian:bookworm-slim
RUN useradd --system --create-home --uid 10001 whidictate && mkdir /data && chown whidictate /data
COPY --from=build /src/target/release/whidictate-server /usr/local/bin/whidictate-server
USER whidictate
ENV WHIDICTATE_DATA=/data WHIDICTATE_LISTEN=0.0.0.0:8484 WHIDICTATE_SECURE_COOKIES=1
VOLUME /data
EXPOSE 8484
CMD ["whidictate-server", "serve"]
