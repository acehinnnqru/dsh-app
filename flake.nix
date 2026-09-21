{
  description = "dsh-app — a Tauri launcher for DeepSeek Harness profiles";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    devshells = {
      url = "github:acehinnnqru/devshells";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  # This flake wraps no runtime: there is no closure to build, no Node archive to
  # pin, and no seed to carry. What it produces is a shell binary and the .app
  # around it.
  #
  # The bundle identifier is embedded in Info.plist and therefore in the
  # signature's designated requirement, so it must be stated rather than
  # defaulted to something nobody chose.
  #
  # It is read from the environment, which means app builds need --impure.
  # Flakes cannot take arguments: --arg/--argstr are rejected for flake
  # installables, and a repository file cannot serve either — tracked means not
  # configurable, untracked means it never enters the flake source.
  outputs = {
    nixpkgs,
    utils,
    devshells,
    ...
  }:
    utils.lib.eachSystem ["aarch64-darwin"] (
      system: let
        pkgs = import nixpkgs {inherit system;};
        inherit (nixpkgs) lib;

        rootManifest = builtins.fromJSON (builtins.readFile ./package.json);
        shellVersion = rootManifest.version;

        appId = builtins.getEnv "DSH_APP_APP_ID";
        requireAppId =
          if appId == ""
          then throw ''
            DSH_APP_APP_ID is unset: an app bundle needs a bundle identifier, and
            it becomes part of the signature. It is read from the environment, so
            build with --impure and export it first (.envrc already does):

              nix build --impure .#dsh-app
          ''
          else appId;

        # Apple Silicon macOS is the only target, because that is the only
        # platform the shell has been run on.
        isDarwin = pkgs.stdenv.hostPlatform.isDarwin;

        pnpm = pkgs.pnpm;

        # The composition helpers this repository's devShell has always used.
        ts = devshells.legacyPackages.${system}.toolsets;
        inherit (ts.lib) combine mkShellArgs;

        # The shell. Built directly with cargo rather than through `tauri build`:
        # the frontend is four static files that need no toolchain, and the real
        # GUI is served by the runtime at run time.
        dshApp = pkgs.rustPlatform.buildRustPackage {
          pname = "dsh-app";
          version = shellVersion;
          src = ./.;
          cargoRoot = "src-tauri";
          buildAndTestSubdir = "src-tauri";
          cargoLock.lockFile = ./src-tauri/Cargo.lock;

          nativeBuildInputs =
            [pkgs.pkg-config]
            ++ lib.optionals isDarwin [pkgs.apple-sdk_14];

          # The shell page is embedded; nothing else is compiled.
          # `cargoRoot = "src-tauri"` is where `cargoSetupHook` stages the vendor
          # config, but each phase runs in its own shell, so a path exported by
          # `preBuild` would not survive. Locate the crate in each phase instead.
          buildPhase = ''
            # The build phase runs from the source root; the crate is in
            # `src-tauri` (which is also what `cargoRoot` tells the setup hook).
            cd src-tauri
            cargo build --release --offline
          '';

          installPhase = ''
            mkdir -p "$out/bin"
            binary="$(find . -maxdepth 3 -path '*/target/release/dsh-app' -type f | head -n1)"
            [ -n "$binary" ] || { echo "the shell binary was not built" >&2; exit 1; }
            install -m 755 "$binary" "$out/bin/dsh-app"

            # Rewrite any load command that points into the Nix store to the
            # system library it duplicates, then assert none remain.
            #
            # This is not cosmetic. A store path is an absolute path into *this*
            # machine's Nix store, so a shipped bundle would fail to launch
            # anywhere else; and once the app is signed, the hardened runtime
            # refuses to load a dylib signed by a different team, which every
            # Nix-store library is. Both failures appear only after signing,
            # which is why they went unnoticed until the signature existed.
            #
            # macOS carries the same library in the dyld shared cache, where it
            # counts as a platform binary and is always acceptable.
            for current in $(otool -L "$out/bin/dsh-app" | tail -n +2 | awk '{print $1}' | grep '^/nix/store/' || true); do
              case "$current" in
                */libiconv.2.dylib) install_name_tool -change "$current" /usr/lib/libiconv.2.dylib "$out/bin/dsh-app" ;;
              esac
            done

            remaining="$(otool -L "$out/bin/dsh-app" | tail -n +2 | awk '{print $1}' | grep -c '^/nix/store/' || true)"
            if [ "$remaining" != "0" ]; then
              echo "the shell still references $remaining Nix store libraries:" >&2
              otool -L "$out/bin/dsh-app" | grep '/nix/store/' >&2
              echo "the bundle would not launch elsewhere, and signing would refuse them" >&2
              exit 1
            fi
          '';

          doCheck = false;
          dontFixup = true;
        };

        # A shippable, unsigned .app. Signing is a separate documented step: it
        # needs credentials and cannot be deterministic (design doc §7.2).
        #
        # There is one variant and one only. The shell carries no Node, no pnpm,
        # and no runtime closure, so there is nothing for a second packaging of
        # the same binary to vary.
        app = pkgs.stdenv.mkDerivation {
          pname = "dsh-app";
          version = shellVersion;
          dontUnpack = true;

          DSH_APP_APP_ID = requireAppId;

          buildPhase = ''
            work="$PWD/work"
            mkdir -p "$work/bin" "$work/out"

            # Every destination below names its file explicitly: `cp <file> <dir>/`
            # keeps the SOURCE basename, and a Nix store path is prefixed with its
            # hash, which once produced a bundle asset named after the hash.
            crate="$work/src-tauri"
            mkdir -p "$crate/packaging" "$crate/icons"
            cp ${./src-tauri/packaging/assemble-app.sh} "$crate/packaging/assemble-app.sh"
            chmod +x "$crate/packaging/assemble-app.sh"
            cp ${./src-tauri/icons/icon.icns} "$crate/icons/icon.icns"
            install -m 755 ${dshApp}/bin/dsh-app "$work/bin/dsh-app"

            "$crate/packaging/assemble-app.sh" \
              --binary "$work/bin/dsh-app" \
              --out "$work/out" \
              --bundle-id ${requireAppId} \
              --version ${shellVersion}
          '';

          installPhase = ''
            mkdir -p "$out/Applications"
            cp -R "$PWD/work/out/dsh-app.app" "$out/Applications/"
          '';

          dontFixup = true;
        };
      in {
        packages = {
          # The shell and the bundle it lives in. One product, one name.
          dsh-app = app;
          default = app;
        };

        devShells.default = pkgs.mkShell (mkShellArgs (combine [
          ts.base

          ## Rust
          ts.rust.stable
          # ts.rust.nightly
          # ts.rust.nightly-wasm

          ## Go
          # ts.go.latest
          # ts.go."1_25"
          # ts.go."1_24"

          ## Node.js
          ts.nodejs."22"

          ## Python
          # ts.python."314"
          # ts.python."313"
          # ts.python."311"

          ## Zig
          # ts.zig.default
          # ts.zig.latest

          ## Lua
          # ts.lua.default

          ## Java
          # ts.java.android

          ## Thrift
          # ts.thrift.default

          ## Project-specific
          {
            packages = [
              pkgs.pkg-config
              # The Tauri CLI and the dev tooling are Node programs; pnpm runs
              # them. `packageManager` in package.json is deliberately absent: it
              # makes pnpm re-exec a different version it then has to download,
              # which in a sandbox with no network fails silently.
              pnpm
            ] ++ lib.optionals isDarwin [
              # Tauri/wry links the system WKWebView and these frameworks.
              # NOTE: `pkgs.darwin.apple_sdk.frameworks.*` is gone in the pinned
              # nixpkgs (legacy stub removed); the SDK ships as `apple-sdk_*`.
              pkgs.apple-sdk_14
            ];

            shellHook = ''
              export PATH="$PWD/node_modules/.bin:$PATH"
              echo "dsh-app devshell: node $(node --version), pnpm $(pnpm --version), $(cargo --version)"
            '';
          }
        ]));
      }
    );
}
