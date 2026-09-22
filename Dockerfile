# Container image with both binaries: the server and the teacher CLI.
#
#   docker build -t quiz .
#   docker run --rm -p 8080:8080 --env-file deploy/quiz-server.env quiz

FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked --package quiz-server --package quiz-cli

FROM debian:bookworm-slim
# TLS to the OIDC provider needs the CA bundle; rustls brings the rest.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 quiz \
    && install -d -o quiz -g quiz /var/log/quiz

COPY --from=build /src/target/release/quiz-server /usr/local/bin/quiz-server
COPY --from=build /src/target/release/quiz /usr/local/bin/quiz

USER quiz
ENV QUIZ_LISTEN=0.0.0.0:8080 \
    QUIZ_LOG_FILE=/var/log/quiz/quiz-server.log
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/quiz-server"]
