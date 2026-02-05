{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = inputs @ { self, ... }:
    (inputs.flake-utils.lib.eachDefaultSystem (system:
      let

        pkgs = import inputs.nixpkgs {
          inherit system;
        };

        inherit (builtins) readFile fromTOML;

        claude-chill = pkgs.rustPlatform.buildRustPackage {
          pname = (fromTOML (readFile ./crates/claude-chill/Cargo.toml)).package.name;
          version = (fromTOML (readFile ./Cargo.toml)).workspace.package.version;
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type: !(pkgs.lib.hasSuffix ".nix" path);
          };
          cargoHash = "sha256-mLiOFdIRluTOahLF29by6D+JuHfUW9VRCHNpLxKiIvE=";
        };

      in
      {

        packages = {
          default = claude-chill;

          claude-chill = claude-chill;
        };

      }
    ));
}
