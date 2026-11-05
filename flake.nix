{
  description = "MCRS firmware and development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    esp-rs-nix = {
      url = "git+ssh://git@github.com/leighleighleigh/esp-rs-nix.git?ref=refs/tags/v3.0.1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    elf2flash = {
      url = "github:BjornTheProgrammer/elf2flash";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      esp-rs-nix,
      crane,
      elf2flash,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      perSystem =
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          # Root workspace and platform/nrf52840: channel and components pinned in rust-toolchain.toml,
          # plus the thumbv7em target from platform/nrf52840/rust-toolchain.toml
          stable = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            inherit ((pkgs.lib.importTOML ./platform/nrf52840/rust-toolchain.toml).toolchain) targets;
          };

          # platform/esp32: Xtensa Rust with rust-src, plus the ESP GCC toolchains
          esp = esp-rs-nix.packages.${system}.esp-rs;

          # rustup-style proxy for `cargo` and `rustc`. rust-analyzer loads every linked workspace in
          # one process with one environment, and finds each one's toolchain the rustup way: it runs
          # `rustc --print sysroot` in the workspace, then `cargo` with RUSTUP_TOOLCHAIN set to that
          # sysroot. As with rustup, the toolchain is named by RUSTUP_TOOLCHAIN, else by the
          # `channel` of the nearest rust-toolchain.toml
          rustup-proxy = pkgs.writeShellApplication {
            name = "rustup-proxy";
            text =
           ''
            toolchain=''${RUSTUP_TOOLCHAIN-}
            dir=$PWD
            until [[ -n $toolchain || -z $dir ]]; do
              file=$dir/rust-toolchain.toml
              [[ -f $file && $(<"$file") =~ channel\ *=\ *\"([^\"]+)\" ]] && toolchain=''${BASH_REMATCH[1]}
              dir=''${dir%/*}
            done

            case ''${toolchain:-stable} in
              stable) sysroot=${stable} ;;
              esp) sysroot=${esp} ;;
              # rust-analyzer passes the sysroot itself
              /*) sysroot=$toolchain ;;
              *) echo "''${0##*/}: no toolchain '$toolchain' in this dev shell (see flake.nix)" >&2; exit 1 ;;
            esac

            # Keep what it runs (rustc, cargo subcommands, the linker) on the same toolchain
            export PATH=$sysroot/bin:$PATH
            exec "$sysroot/bin/''${0##*/}" "$@"
          '';
          };

          # `cargo` and `rustc` through the proxy, the rest as is: stable's rust-analyzer, rustfmt etc.
          # (cargo still runs each toolchain's own subcommands) and the ESP GCC toolchains
          rust = pkgs.runCommand "rust-toolchains" { } ''
            mkdir -p $out/bin
            ln -s ${rustup-proxy}/bin/rustup-proxy $out/bin/cargo
            ln -s ${rustup-proxy}/bin/rustup-proxy $out/bin/rustc
            for bin in ${stable}/bin/* ${esp}/bin/*; do
              [[ -e $out/bin/''${bin##*/} ]] || ln -s "$bin" $out/bin/
            done
          '';

          # The version the firmware reports, as repeater/build.rs makes it from git. Nix builds from a
          # copy of the tree without .git, so the commit is passed in
          commit =
            if self ? rev then
              lib.substring 0 8 self.rev
            else if self ? dirtyRev then
              lib.substring 0 8 self.dirtyRev + ".dirty"
            else
              null;
          cargoVersion = (lib.importTOML ./repeater/Cargo.toml).package.version;
          version = cargoVersion + lib.optionalString (commit != null) "+${commit}";

          # The toolchains, by the channel that a workspace's rust-toolchain.toml names
          toolchains = { inherit stable esp; };

          craneLib = crane.mkLib pkgs;

          # Builds `bin` from the Cargo workspace in platform/${platform}, as $out/${name}.elf, with the
          # toolchain and target from its rust-toolchain.toml and .cargo/config.toml. crane builds the
          # dependencies first, from the source with the project's own crates stubbed out, so changing
          # the firmware doesn't rebuild them and boards on one platform share them. `profile` is
          # applied after profiles/defaults.conf, as with the Makefile's PROFILE
          mkFirmware =
            {
              name,
              pname ? name,
              platform,
              bin,
              features ? [ ],
              profile ? null,
            }:
            let
              cargoConfig = lib.importTOML ./platform/${platform}/.cargo/config.toml;
              toolchain =
                toolchains.${(lib.importTOML ./platform/${platform}/rust-toolchain.toml).toolchain.channel};
              platformCraneLib = craneLib.overrideToolchain (_: toolchain);

              # Only what the workspace builds from, so changing anything else doesn't rebuild it
              src = lib.fileset.toSource {
                root = ./.;
                fileset = lib.fileset.unions [
                  ./board
                  ./platform
                  ./profiles
                  ./protocol
                  ./repeater
                ];
              };

              # The crates in the lock files, fetched by Nix so that Cargo can build offline. build-std
              # also builds the standard library, from the toolchain's own lock file, and reading that
              # builds the toolchain while evaluating
              cargoVendorDir = platformCraneLib.vendorMultipleCargoDeps {
                cargoLockList = [
                  ./platform/${platform}/Cargo.lock
                ]
                ++ lib.optional (
                  cargoConfig ? unstable.build-std
                ) "${toolchain}/lib/rustlib/src/rust/library/Cargo.lock";
              };

              featureFlags = lib.optionals (features != [ ]) [
                "--features"
                (lib.concatStringsSep "," features)
              ];

              commonArgs = {
                inherit src cargoVendorDir;
                # The dependency build's stubbed-out source has no Cargo.lock in the workspace, which
                # isn't at the root of the source, so crane copies this one in
                cargoLock = ./platform/${platform}/Cargo.lock;
                # The tests run on the host, from the root workspace
                doCheck = false;
                # rustc writes source paths into the firmware, for panic locations and debug info.
                # crane removes the vendored crates' store paths from what it installs, and this remaps
                # the toolchain's: unlike RUSTFLAGS, a `target` table adds to the workspace's own
                # rustflags. Then Cargo runs in the workspace, which isn't at the root of the source
                postUnpack = ''
                  mkdir -p $sourceRoot/.cargo
                  echo "target.'cfg(all())'.rustflags = [\"--remap-path-prefix=${toolchain}=/toolchain\"]" > $sourceRoot/.cargo/config.toml
                  cd $sourceRoot/platform/${platform}
                  sourceRoot=.
                '';
              };
            in
            platformCraneLib.buildPackage (
              commonArgs
              // {
                inherit pname version;

                cargoArtifacts = platformCraneLib.buildDepsOnly (
                  commonArgs
                  // {
                    # Without the commit, so only changing the dependencies rebuilds them
                    pname = "mcrs-${platform}${lib.concatMapStrings (feature: "-${feature}") features}";
                    version = cargoVersion;
                    # The binaries are stubs here and wouldn't link, so build the dependencies through
                    # the workspace's library
                    buildPhaseCargoCommand = "cargoWithProfile build ${
                      lib.escapeShellArgs ([ "--locked" "--lib" ] ++ featureFlags)
                    }";
                  }
                );

                cargoExtraArgs = lib.escapeShellArgs ([ "--locked" "--bin" bin ] ++ featureFlags);

                # Cargo only rebuilds a crate whose sources are newer than its last build, and sources
                # from the store date from 1970. Dating the project's own sources now (../.. is the root
                # of the source) makes sure they replace the dependency build's stubs
                preBuild = ''
                  find ../.. -name target -prune -o -name '*.rs' -exec touch {} +
                '';

                installPhaseCommand = ''
                  mkdir -p $out
                  cp ''${CARGO_TARGET_DIR:-target}/${cargoConfig.build.target}/release/${bin} $out/${name}.elf
                '';

                # The host's strip and patchelf don't handle firmware for the target
                dontFixup = true;
                # Firmware runs on the board, so it shouldn't keep anything from the store
                allowedReferences = [ ];
              }
              # Read by repeater/build.rs
              // lib.optionalAttrs (commit != null) { MCRS_GIT_COMMIT = commit; }
              // lib.optionalAttrs (profile != null) { MCRS_PROFILE = "${profile}"; }
            );

          # Names a build with a profile as the Makefile does, e.g. -nemesh for nemesh.conf
          profileSuffix =
            profile:
            lib.optionalString (profile != null) "-${lib.removeSuffix ".conf" (baseNameOf (toString profile))}";

          # A Heltec board, with the images of the Makefile's *-bins targets: a merged image to flash
          # over USB at 0x0 and an upgrade image for OTA updates. The ELF is for decoding defmt logs
          heltecFirmware = lib.makeOverridable (
            {
              board,
              flashSize,
              mqtt ? false,
              profile ? null,
            }:
            let
              name = "mcrs-${board}${profileSuffix profile}${lib.optionalString mqtt "-mqtt"}";
              elf = mkFirmware {
                inherit name profile;
                pname = "${name}-elf";
                platform = "esp32";
                bin = "mcrs-${lib.replaceStrings [ "-" ] [ "_" ] board}";
                features = lib.optional mqtt "mqtt";
              };
            in
            # crane edits the ELF it installs to remove store paths, so the images are made from the
            # finished ELF, in a derivation of their own: nothing changes them after espflash
            # checksums them
            pkgs.runCommand "${name}-${version}"
              {
                nativeBuildInputs = [ pkgs.espflash ];
                allowedReferences = [ ];
              }
              ''
                mkdir -p $out
                cp ${elf}/${name}.elf $out/
                flags=(--skip-update-check --chip esp32s3 --flash-size ${flashSize} --flash-mode dio --flash-freq 40mhz --partition-table ${./platform/esp32/partitions.csv})
                espflash save-image "''${flags[@]}" --merge $out/${name}.elf $out/${name}-merged.bin
                espflash save-image "''${flags[@]}" --target-app-partition ota_0 $out/${name}.elf $out/${name}-upgrade.bin
              ''
          );

          # An nRF52840 board, as a UF2 image for its UF2 bootloader, made from the finished ELF like
          # the Heltec images
          nrfFirmware = lib.makeOverridable (
            {
              board,
              profile ? null,
            }:
            let
              name = "mcrs-${board}${profileSuffix profile}";
              elf = mkFirmware {
                inherit name profile;
                pname = "${name}-elf";
                platform = "nrf52840";
                bin = "mcrs-${board}";
              };
            in
            pkgs.runCommand "${name}-${version}"
              {
                nativeBuildInputs = [ elf2flash.packages.${system}.default ];
                allowedReferences = [ ];
              }
              ''
                mkdir -p $out
                elf2flash convert --family 0xada52840 ${elf}/${name}.elf $out/${name}.uf2
              ''
          );

          # The nRF52840 boards, one for each binary in platform/nrf52840 (`mcrs-<board>`), so adding a
          # board there adds its package
          nrfBoards = map (lib.removePrefix "mcrs-") (
            lib.catAttrs "name" (lib.importTOML ./platform/nrf52840/Cargo.toml).bin
          );

          # The board packages take `.override { profile = "nemesh.conf"; }`, and the Heltec ones `mqtt`
          boards = {
            heltec-v3 = heltecFirmware {
              board = "heltec-v3";
              flashSize = "8mb";
            };
            heltec-v4 = heltecFirmware {
              board = "heltec-v4";
              flashSize = "16mb";
            };
            heltec-wsl3 = heltecFirmware {
              board = "heltec-wsl3";
              flashSize = "8mb";
            };
          }
          // lib.genAttrs nrfBoards (board: nrfFirmware { inherit board; });

          packages = boards // {
            heltec-v3-mqtt = boards.heltec-v3.override { mqtt = true; };
            heltec-v4-mqtt = boards.heltec-v4.override { mqtt = true; };
            heltec-wsl3-mqtt = boards.heltec-wsl3.override { mqtt = true; };

            # Every board
            default = pkgs.symlinkJoin {
              name = "mcrs-firmware-${version}";
              paths = lib.attrValues boards;
            };
          };
        in
        {
          inherit packages;

          # One dev shell for all three workspaces, each gets its toolchain from `rustup-proxy`
          devShells.default = pkgs.mkShell {
            packages = [
              rust

              pkgs.cargo-binutils
              # Runner for `cargo run` in platform/* (see .cargo/config.toml)
              pkgs.probe-rs-tools
              # Image and flashing tool for the Heltec targets (see Makefile)
              pkgs.espflash
            ];
          };
        };

      systems = lib.genAttrs [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ] perSystem;
    in
    {
      packages = lib.mapAttrs (_: outputs: outputs.packages) systems;
      devShells = lib.mapAttrs (_: outputs: outputs.devShells) systems;
    };
}
