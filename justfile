# Default recipe: list available commands
default:
    @just --list

# --- Development ---

# Start Tauri development server (uses project-root procedures/ by default)
dev *args='':
    pnpm tauri dev -- -- {{ justfile_directory() }}/procedures {{ args }}

# Start frontend-only dev server
dev-frontend:
    pnpm dev

# --- Build ---

# Build the full Tauri application
build:
    pnpm tauri build

# Build frontend only
build-frontend:
    pnpm build

# --- Check & Lint ---

# Run all checks (fmt, clippy, vp, svelte-check, type bindings, tests)
check-all: check-fmt check-clippy check-vp check-frontend check-types test

# Check Rust formatting
check-fmt:
    cargo fmt --all -- --check

# Run cargo clippy, denying warnings for local packages
check-clippy:
    CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features --keep-going

# Run vite-plus check (format + lint)
check-vp:
    pnpm run vp:check

# Run svelte-check
check-frontend:
    pnpm check

# --- Format ---

# Format Rust code
fmt:
    cargo fmt --all

# --- Type Bindings ---

# Generate TypeScript type bindings from Rust types
generate-types:
    cargo test --workspace export_bindings_

# Check that generated TypeScript types are up-to-date
check-types:
    cargo test --workspace export_bindings_
    pnpm exec vp fmt src/lib/types/generated/
    git diff --exit-code src/lib/types/generated/

# --- Test ---

# Run all tests
test: test-rust

# Run Rust tests
test-rust:
    cargo test --workspace

# --- Utility ---

# Run pre-commit hooks on all files
pre-commit:
    pre-commit run --all-files

# Clean build artifacts
clean:
    cargo clean
    rm -rf build .svelte-kit node_modules/.vite
