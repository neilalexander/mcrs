{
  description = "Development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    esp-rs-nix = {
      url = "git+ssh://git@github.com/leighleighleigh/esp-rs-nix.git";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, esp-rs-nix, ... }:
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
          pkgs = nixpkgs.legacyPackages.${system};
          # Xtensa Rust with rust-src, plus the ESP GCC toolchains
          esp-rs = esp-rs-nix.packages.${system}.esp-rs;
          # Every crate builds with the ESP toolchain. The Makefile selects it with
          # `cargo +esp`, a rustup toolchain name, so accept and drop that argument.
          cargo = pkgs.writeShellScriptBin "cargo" ''
            if [ "$1" = "+esp" ]; then
              shift
              export PATH="${esp-rs}/bin:$PATH"
              export RUSTC="${esp-rs}/bin/rustc"
              exec ${esp-rs}/bin/cargo "$@"
            fi
            exec ${esp-rs}/bin/cargo "$@"
          '';
        in
        {
          default = pkgs.mkShell {
            packages = [
              # Listed first so it shadows the toolchain's own cargo
              #cargo
              esp-rs
              # Runner for `cargo run` (see .cargo/config.toml)
              pkgs.probe-rs-tools
              # Image and flashing tool for the Heltec targets (see Makefile)
              pkgs.espflash
            ];
          };
        }
      );
    };
}
