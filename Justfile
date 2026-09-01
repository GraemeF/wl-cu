# wl-cu task runner

default:
    @just --list

build:
    cargo build --release

install:
    cargo install --path . --locked --force

check: fmt clippy test

_clippy-strict:
    cargo clippy -- -D warnings

clippy:
    cargo clippy --fix --allow-dirty --allow-staged --allow-no-vcs

test:
    cargo test

fmt:
    cargo fmt

# Launch nested compositors inside the current Wayland session.
nested: nested-niri

nested-niri:
    niri -c test/niri-test-config.kdl

nested-hyprland:
    Hyprland --config test/hyprland-test-config.conf
