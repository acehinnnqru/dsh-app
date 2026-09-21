# AGENTS.md

## Project notes

- Run `nix develop --command cargo build` before changing anything. **The baseline is green**; any error is newly introduced.
- **The shell ships no runtime.** No Node, no pnpm, no closure, no update channel: it supervises the `dsh` executable the user names. Anything that looks like packaging a runtime belongs in the user's own dsh installation, not here — see `docs/spec-external-dsh.md`.
- There is one product and one name (`.#dsh-app`). There is no `bundled`/`slim` variant to keep in sync.
- `cargo test` covers the resolution chain, profile discovery, and the startup contract. Note that in this environment a failing test aborts with `failed to initiate panic` instead of printing a summary unless you raise `RUST_MIN_STACK`; the panic line itself is still printed, so read it.
