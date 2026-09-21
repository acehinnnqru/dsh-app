# dsh-app

A Tauri launcher for DeepSeek Harness (dsh) profiles.

The app ships **no runtime of its own**. It supervises the `dsh` executable you
point it at, so:

1. install dsh (however you normally do — npm, nix, a package manager);
2. run `which dsh` and paste that path into the form on first launch.

Both values are stored in `~/.config/dsh-app/config.toml` (`$XDG_CONFIG_HOME` is
honoured), which is the shell's own file — so `DSH_HOME=$(mktemp -d) dsh-app`
isolates the harness home without making the app forget which dsh you use.

The harness home defaults to `~/.dsh` and is shared with the CLI, so profiles,
sessions, and credentials are the same in both, and plugins and updates stay
owned by your dsh installation.

## Working on it

```sh
use flake .                                    # the devshell (direnv does this here)
cargo run --manifest-path src-tauri/Cargo.toml # run the shell
nix build --impure .#dsh-app                   # the .app (DSH_APP_APP_ID comes from .envrc)
```

`docs/spec-external-dsh.md` records what the shell does and does not own;
`docs/design-tauri-wrapper.md` is the design behind it.
