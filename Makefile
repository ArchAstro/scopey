.PHONY: all build build-release install uninstall test test-all clean fmt lint check \
	setup setup-hooks doctor verify-models release-check help

# Default target
all: build

# Debug build
build:
	cargo build

# Release build
build-release:
	cargo build --release

# Install to ~/.cargo/bin (or CARGO_HOME/bin)
install: build-release
	cargo install --path . --force

# Validate a release candidate. Tagging and pushing stay explicit human actions.
release-check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets --all-features -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
	cargo test --locked --all-targets --all-features

# Run unit tests
test:
	cargo test

# Full test suite
test-all:
	cargo test --all-targets --all-features

# Clean build artifacts and local scratch (not ~/.scopey)
clean:
	cargo clean
	rm -rf .scopey/

# Format
fmt:
	cargo fmt

# Lint
lint:
	cargo clippy --all-targets --all-features -- -D warnings

# Fast compile check without codegen
check:
	cargo check

# Dev setup: release binary + default config + harness hooks
setup: build-release setup-hooks
	@echo "scopey ready: $$(pwd)/target/release/scopey"
	@echo "Also on PATH after: make install"

# Install/refresh Claude + Codex hooks and default config
setup-hooks:
	cargo run --quiet --release -- setup --force

# Remove scopey hooks (keep ~/.scopey data). Use: make uninstall PURGE=1 for full wipe.
uninstall:
	cargo run --quiet --release -- uninstall $(if $(PURGE),--purge-data,)

# Health check (binary, config, runners, hooks, notifications)
doctor:
	cargo run --quiet --release -- doctor

# Probe shipped fast models for claude + codex runners
verify-models:
	cargo run --quiet --release -- models --verify

help:
	@echo "scopey Makefile targets (mirrors archastro/aster conventions):"
	@echo "  make / make build     debug cargo build"
	@echo "  make build-release    optimized binary in target/release/scopey"
	@echo "  make install          cargo install --path . --force"
	@echo "  make test             cargo test"
	@echo "  make test-all         all targets + features"
	@echo "  make check            cargo check"
	@echo "  make fmt / make lint  format / clippy -D warnings"
	@echo "  make clean            cargo clean + rm -rf .scopey/"
	@echo "  make setup            release build + install hooks"
	@echo "  make setup-hooks      scopey setup --force"
	@echo "  make uninstall        remove hooks (PURGE=1 also deletes ~/.scopey)"
	@echo "  make doctor           scopey doctor"
	@echo "  make verify-models    probe claude/codex fast model defaults"
	@echo "  make release-check    fmt + clippy + doc + test (pre-release)"
