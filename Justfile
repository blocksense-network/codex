#
# Nix Dev Shell Policy (reproducibility)
# -------------------------------------
# When running inside the Nix dev shell (`IN_NIX_SHELL` is set), Just tasks and
# helper scripts must avoid fallbacks like network installs or ad-hoc bootstraps.
# If a required command is missing in that context, add it to `flake.nix`
# (devShell.buildInputs) and re-enter the shell instead of falling back. Outside
# the dev shell, convenience fallbacks are acceptable as long as they abort when
# running under Nix.

set positional-arguments
set shell := ["./scripts/nix-env.sh", "-c"]

default:
    just help

help:
    just -l

# Enter the flake's development shell manually
shell:
    nix develop

# Node / repo maintenance ----------------------------------------------------

pnpm-install:
    pnpm install

pnpm-format:
    pnpm format

pnpm-format-fix:
    pnpm format:fix

# Rust workspace convenience wrappers ---------------------------------------

rust-fmt:
    just --justfile codex-rs/justfile fmt

rust-install:
    just --justfile codex-rs/justfile install

rust-test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    just --justfile codex-rs/justfile test "$@"

rust-codex *args:
    #!/usr/bin/env bash
    set -euo pipefail
    just --justfile codex-rs/justfile codex "$@"

rust-tui *args:
    #!/usr/bin/env bash
    set -euo pipefail
    just --justfile codex-rs/justfile tui "$@"

rust-exec *args:
    #!/usr/bin/env bash
    set -euo pipefail
    just --justfile codex-rs/justfile exec "$@"
