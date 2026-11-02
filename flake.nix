{
  description = "Development shell";

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
  };

  outputs =
    { nixpkgs, rust-overlay, esp-rs-nix, ... }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
    in
    {
      devShells = forAllSystems (
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
        in
        {
          # One dev shell for all three workspaces, each gets its toolchain from `rustup-proxy`
          default = pkgs.mkShell {
            packages = [
              rust
              # Runner for `cargo run` in platform/* (see .cargo/config.toml)
              pkgs.probe-rs-tools
              # Image and flashing tool for the Heltec targets (see Makefile)
              pkgs.espflash
            ];
          };
        }
      );
    };
}
