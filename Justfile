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

# Tests run with nextest, not `cargo test`.
test:
    cargo nextest run --workspace --all-features

fmt:
    cargo fmt --all

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
