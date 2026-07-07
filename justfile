# nobrew task runner (2lab.ai / herdr conventions)

# Format + lint + test — the gate before every commit
check:
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked

# Run tests
test:
    cargo test --locked

# Build release binary
build:
    cargo build --release --locked

# Format the tree
fmt:
    cargo fmt

# Cut a stable release: tag v<Cargo.toml version> and push (fires release.yml)
release:
    #!/usr/bin/env bash
    set -euo pipefail
    v=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
    git tag "v$v"
    git push origin "v$v"
    echo "pushed v$v — release.yml will build and publish binaries"
