{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };
          inherit (pkgs) lib;

          # Read rust version from rust-version file
          rustVersion = lib.strings.removeSuffix "\n" (builtins.readFile ./rust-version);

          rustToolchain = pkgs.rust-bin.stable.${rustVersion}.default.override {
            extensions = [ "rust-src" "clippy" ];
          };

          nativeBuildInputs = with pkgs; [ rustToolchain pkg-config ];
          buildInputs = with pkgs; [ openssl ];

        in
        {
          devShells.default = pkgs.mkShell {
            inherit buildInputs nativeBuildInputs;

            RUST_VERSION = rustVersion;
          };
        }
      );
} 