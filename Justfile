# niri-cu task runner

# Default recipe
default:
    @just --list

# Build release binary
build:
    cargo build --release

# Install to ~/.cargo/bin
install:
    cargo install --path . --locked --force

# Run all post-change checks
check: fmt clippy test

# Clippy with denied warnings
_clippy-strict:
    cargo clippy -- -D warnings

# Run clippy and apply machine-applicable fixes
clippy:
    cargo clippy --fix --allow-dirty --allow-staged --allow-no-vcs

# Run tests
test:
    cargo test

# Format code
fmt:
    cargo fmt

# Launch a nested niri session for testing (runs as a window in the host session).
# The nested instance logs its own WAYLAND_DISPLAY and NIRI_SOCKET on startup;
# export both before pointing niri-cu (or test apps) at it.
nested:
    niri -c test/niri-test-config.kdl
