# AGENTS.md

## Project notes

- Run `nix develop --command cargo build` before changing anything. **The baseline is green**; any error is newly introduced.
- **Dev builds probe PATH for node.** Verifying the "unconfigured" flow requires `cargo build --release`.
- The `slim` variant never guesses Node paths: no PATH scan, no standard install locations (dev builds only are the exception).
