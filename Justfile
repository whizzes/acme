target_release := "x86_64-unknown-linux-musl"

default:
    @echo "No default task specified"
    just --list

dkc:
    docker pull ghcr.io/leoborai/dkc:latest
    docker run -it --rm \
        -v $(pwd):/app \
        -w /app \
        ghcr.io/leoborai/dkc:latest

# Server. Binary is `acme` (crate acme-server); CLI subcommands (seed, reset,
# openapi, migrate) land progressively — see specs/000-Initial-Spec.md §19/§20.
run:
    ACME_LOG=info,acme=debug cargo run -p acme-server

watch:
    cargo watch -x 'run -p acme-server'

seed scale='medium':
    cargo run -p acme-server -- seed --scale {{scale}}

reset:
    cargo run -p acme-server -- reset --yes

openapi:
    cargo run -p acme-server -- openapi --out docs/openapi

build-release version target=target_release:
    @echo "Release Build on {{target}} with {{version}}"
    cargo zigbuild --release \
        --bin acme \
        -p acme-server \
        --target {{target}}

build-docker-image version target=target_release:
    @echo "Building Docker Image for {{version}}-{{target}}"
    ./docker/docker-build.sh {{version}} {{target}} target/{{target}}/release/acme

push-docker-image version registry target=target_release:
    @echo "Pushing Docker Image for {{version}}-{{target}}"
    docker tag acme:{{version}}-{{target}} {{registry}}/acme:{{version}}-{{target}}
    docker push {{registry}}/acme:{{version}}-{{target}}

# Tests run with nextest, not `cargo test`.
test:
    cargo nextest run --workspace --all-features

fmt:
    cargo fmt --all

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo run -p xtask -- spec-lint

spec-lint:
    cargo run -p xtask -- spec-lint
