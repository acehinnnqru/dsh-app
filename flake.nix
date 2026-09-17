{
  description = "dsh-app — a Tauri desktop shell for DeepSeek Harness";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    devshells = {
      url = "github:acehinnnqru/devshells";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  # `appId` is an argument rather than an environment lookup: flake evaluation is
  # pure, and the identifier becomes part of the code signature, so it must be
  # stated explicitly and appear in the build it produced. Build the app with
  # `nix build --argstr appId com.example.dsh-app .#dsh-app-bundled`.
  # Apple Silicon macOS is the only target. Declaring it here means evaluating
  # this flake anywhere else fails with "flake does not provide" rather than
  # producing an artifact tagged for the wrong platform — the closure resolves
  # native packages (`node-pty`, `koffi`, the `require-builtin` addon) for the
  # machine that builds it, so there is no correct cross-build.
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

        # The single source of truth for what this app wraps.
        rootManifest = builtins.fromJSON (builtins.readFile ./package.json);
        dshVersion = rootManifest.dshRuntime.version;
        dshPackage = rootManifest.dshRuntime.package;
        nodeVersion = rootManifest.dshRuntime.nodeVersion;
        shellVersion = rootManifest.version;

        # The bundle identifier is embedded in `Info.plist` and therefore in the
        # signature's designated requirement, so it must be stated rather than
        # defaulted to something nobody chose.
        #
        # It is read from the environment, which means app builds need `--impure`.
        # Flakes cannot take arguments: `--arg`/`--argstr` are rejected for flake
        # installables in Nix 2.34 ("incompatible with flakes"), and a repository
        # file cannot serve either — tracked means not configurable, untracked
        # means it never enters the flake source.
        appId = builtins.getEnv "DSH_APP_APP_ID";
        requireAppId =
          if appId == ""
          then throw ''
            DSH_APP_APP_ID is unset: an app bundle needs a bundle identifier, and
            it becomes part of the signature. It is read from the environment, so
            build with `--impure` and export it first (`.envrc` already does):

              nix build --impure .#dsh-app-bundled
          ''
          else appId;

        # Always true now that the flake declares one system; the conditionals
        # using it are kept because they mark which inputs exist *because of*
        # macOS, which is what a future Linux port would have to revisit.
        isDarwin = pkgs.stdenv.hostPlatform.isDarwin;
        # Node's own arch spelling: `arm64`, not Nix's `aarch64`. The same tag
        # appears in `runtime.json`, the Node archive names, and
        # `channel::platform_tag`; a second convention would silently stop
        # matching.
        platformTag = "${pkgs.stdenv.hostPlatform.parsed.kernel.name}-arm64";

        nodejs = pkgs.nodejs_22;
        # Pinned explicitly: the closure recipe depends on pnpm's deploy
        # semantics, so the flake must name the version it was verified against
        # rather than inherit whatever the caller's PATH happens to offer.
        pnpm = pkgs.pnpm;

        # The composition helpers this repository's devShell has always used.
        ts = devshells.legacyPackages.${system}.toolsets;
        inherit (ts.lib) combine mkShellArgs;

        # The dsh runtime closure, built by the same script the repository uses.
        #
        # Reusing `scripts/prepare-runtime.mjs` instead of restating the pnpm
        # deploy flags here is deliberate: those flags (hoisted linker, peers
        # installed for real) are exactly the part that would silently rot in a
        # second copy.
        #
        # Reproducibility caveat: this derivation is NOT yet a fixed-output
        # derivation, so it resolves from the network at build time. Pinning it
        # (fetchPnpmDeps + an offline install) is tracked as M7 follow-up work —
        # see docs/design-tauri-wrapper.md §16.
        dshRuntime =
          pkgs.stdenv.mkDerivation {
            pname = "dsh-runtime";
            version = dshVersion;
            src = ./.;

            nativeBuildInputs = [nodejs pnpm pkgs.cacert];

            # The closure recipe writes next to the repository; point it at the
            # build directory instead.
            DSH_BUILD_ROOT = "build";
            # pnpm needs a home for its store and metadata inside the sandbox.
            HOME = "home";
            npm_config_store_dir = "store";
            # `CI=1` is set by the Nix build environment and changes pnpm's
            # interactive behaviour; the recipe is written for a plain run.
            CI = "false";

            buildPhase = ''
              export HOME="$PWD/home"
              mkdir -p "$HOME" "$PWD/build"
              echo "pnpm $(pnpm --version), node $(node --version)"
              node scripts/prepare-runtime.mjs --version ${dshVersion}
            '';

            installPhase = ''
              closure="$(find build/runtimes -maxdepth 1 -mindepth 1 -type d | head -n1)"
              [ -n "$closure" ] || { echo "no closure was produced" >&2; exit 1; }
              mkdir -p "$out"
              cp -R "$closure/." "$out/"
            '';

            dontFixup = true;
            dontStrip = true;
            requiredSystemFeatures = ["big-parallel"];
          };

        # The official Node the shell bundles.
        #
        # Only an official build provides `require-builtin` — the addon matches
        # the running binary's machine code, so a distro-repackaged Node loads it
        # and then refuses every id (design doc §11.3). Hence nodejs.org and not
        # a nixpkgs Node.
        #
        # The tarball is hashed by `fetchurl`; the upstream `SHASUMS256.txt` value
        # was checked against it once and matches, so both the transport and the
        # content are accounted for.
        officialNode = pkgs.stdenv.mkDerivation {
          pname = "dsh-app-node";
          version = nodeVersion;
          src = pkgs.fetchurl {
            url = "https://nodejs.org/dist/v${nodeVersion}/node-v${nodeVersion}-${platformTag}.tar.gz";
            hash = "sha256-T8MmajcC7rw5zDdmHPTuzureMH4kKrZOTXznlJGX4R8=";
          };
          # The archive already has the layout we want; strip the version prefix
          # and drop what a runtime does not need.
          sourceRoot = "node-v${nodeVersion}-${platformTag}";
          dontConfigure = true;
          dontBuild = true;
          installPhase = ''
            mkdir -p "$out"
            cp -R bin lib include share "$out/" 2>/dev/null || true
            rm -rf "$out/include" "$out/share"
          '';
          dontFixup = true;
        };

        # The shell. Built directly with cargo rather than through `tauri build`:
        # the frontend is three static files that need no toolchain, and the real
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

          # The shell page and the presets are embedded; nothing else is compiled.
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

        # A shippable, unsigned `.app`. Signing is a separate documented step: it
        # needs credentials and cannot be deterministic (design doc §7.2).
        mkApp = {
          name,
          withNode,
          withSeed,
        }:
          pkgs.stdenv.mkDerivation {
            pname = name;
            version = shellVersion;
            dontUnpack = true;

            nativeBuildInputs = [pkgs.rsync];
            DSH_APP_APP_ID = requireAppId;

            buildPhase = ''
              work="$PWD/work"
              mkdir -p "$work/bin" "$work/out"

              # Every destination below names its file or directory explicitly.
              # `cp <file> <dir>/` keeps the SOURCE basename, and a Nix store path
              # is prefixed with its hash — which silently produced
              # `1sc98ak...-assemble-app.sh` and, worse, a seed runtime directory
              # named after the hash instead of the version the shell looks for.
              crate="$work/src-tauri"
              mkdir -p "$crate/packaging" "$crate/presets" "$crate/icons"
              cp ${./src-tauri/packaging/assemble-app.sh} "$crate/packaging/assemble-app.sh"
              chmod +x "$crate/packaging/assemble-app.sh"
              cp -R ${./src-tauri/presets}/. "$crate/presets/"
              cp ${./src-tauri/icons/icon.icns} "$crate/icons/icon.icns"
              install -m 755 ${dshApp}/bin/dsh-app "$work/bin/dsh-app"

              seed=""
              ${lib.optionalString (withSeed != null) ''
                # `runtime.json` names the version; the directory must match it,
                # because the shell installs the seed under that name.
                seed="$work/seed"
                mkdir -p "$seed"
                cp -R ${withSeed}/. "$seed/"
              ''}

              "$crate/packaging/assemble-app.sh" \
                --binary "$work/bin/dsh-app" \
                --out "$work/out" \
                --bundle-id ${requireAppId} \
                ${lib.optionalString (withSeed != null) ''--seed "$seed" \''}
                --version ${shellVersion}

              ${lib.optionalString withNode ''
                # The bundled Node sits beside the shell executable: sidecar
                # resolution is relative to the executable's directory, and
                # Tauri's bundler only signs inside MacOS.
                mkdir -p "$work/out/dsh-app.app/Contents/Resources/node"
                cp -R ${officialNode}/. "$work/out/dsh-app.app/Contents/Resources/node/"
              ''}
            '';

            installPhase = ''
              mkdir -p "$out/Applications"
              cp -R "$PWD/work/out/dsh-app.app" "$out/Applications/"
            '';

            dontFixup = true;
          };
      in {
        packages = {
          # The runtime closure alone: usable by CI that packages the app itself.
          dsh-runtime = dshRuntime;
          # The official Node archive, pinned by hash.
          node = officialNode;
          # Shell + seed runtime, relying on the user's own Node (design doc D2).
          dsh-app-slim = mkApp {
            name = "dsh-app-slim";
            withNode = false;
            withSeed = dshRuntime;
          };
          # Shell + seed runtime + bundled official Node.
          dsh-app-bundled = mkApp {
            name = "dsh-app-bundled";
            withNode = true;
            withSeed = dshRuntime;
          };
          default = mkApp {
            name = "dsh-app-bundled";
            withNode = true;
            withSeed = dshRuntime;
          };
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
              pkgs.rsync
              # The same pnpm the packaging derivations use. `packageManager` in
              # package.json is deliberately absent: it makes pnpm re-exec a
              # different version it then has to download, which in a sandbox
              # with no network fails silently (see §16).
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
