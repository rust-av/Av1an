precommit:
    cargo +nightly fmt
    cargo clippy --profile ci --tests --benches -- -D warnings
    cargo test --profile ci -p av1an-core
    cargo test --profile ci -p av1an