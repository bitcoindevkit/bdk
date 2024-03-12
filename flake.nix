{
  description = "BDK Flake to run all tests locally and in CI";

  inputs = {
    # stable nixpkgs (let's not YOLO on unstable)
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        flake-utils.follows = "flake-utils";
      };
    };

    flake-utils.url = "github:numtide/flake-utils";

    pre-commit-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, pre-commit-hooks, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        lib = pkgs.lib;
        stdenv = pkgs.stdenv;
        isDarwin = stdenv.isDarwin;
        libsDarwin = with pkgs; lib.optionals isDarwin [
          # Additional darwin specific inputs can be set here
          darwin.apple_sdk.frameworks.Security
          darwin.apple_sdk.frameworks.SystemConfiguration
        ];

        # Dependencies
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        pkgs-esplora = pkgs.callPackage ./nix/packages/esplora/esplora-electrs.nix {
          inherit (pkgs.darwin.apple_sdk.frameworks) Security;
        };

        # Signed Commits
        signed-commits = pkgs.writeShellApplication {
          name = "signed-commits";
          runtimeInputs = [ pkgs.git ];
          text = builtins.readFile ./ci/commits_verify_signature.sh;
        };

        # Toolchains
        # latest stable
        stable_toolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };
        # MSRV stable
        msrv_toolchain = pkgs.rust-bin.stable."1.63.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };
        # Nighly for docs
        nightly_toolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);
        # Code coverage
        coverage_toolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
          extensions = [ "llvm-tools-preview" ];
        };

        # Common inputs
        envVars = {
          BITCOIND_EXEC = "${pkgs.bitcoind}/bin/bitcoind";
          ELECTRS_EXEC = "${pkgs-esplora}/bin/esplora";
          CC = "${stdenv.cc.nativePrefix}cc";
          AR = "${stdenv.cc.nativePrefix}ar";
          CC_wasm32_unknown_unknown = "${pkgs.llvmPackages_14.clang-unwrapped}/bin/clang-14";
          CFLAGS_wasm32_unknown_unknown = "-I ${pkgs.llvmPackages_14.libclang.lib}/lib/clang/14.0.6/include/";
          AR_wasm32_unknown_unknown = "${pkgs.llvmPackages_14.llvm}/bin/llvm-ar";
        };
        buildInputs = with pkgs; [
          # Add additional build inputs here
          git
          bitcoind
          pkgs-esplora
          openssl
          openssl.dev
          pkg-config
          curl
          libiconv
          just
        ] ++ libsDarwin;

        # WASM deps
        WASMInputs = with pkgs; [
          # Additional wasm specific inputs can be set here
          llvmPackages_14.clang-unwrapped
          llvmPackages_14.stdenv
          llvmPackages_14.libcxxClang
          llvmPackages_14.libcxxStdenv
        ];

        nativeBuildInputs = with pkgs; [
          # Add additional build inputs here
          python3
        ] ++ lib.optionals isDarwin [
          # Additional darwin specific native inputs can be set here
        ];
      in
      {
        checks = {
          # Pre-commit checks
          pre-commit-check =
            let
              # this is a hack based on https://github.com/cachix/git-hooks.nix/issues/126
              # we want to use our own rust stuff from oxalica's overlay
              _rust = pkgs.rust-bin.stable.latest.default;
              rust = pkgs.buildEnv {
                name = _rust.name;
                inherit (_rust) meta;
                buildInputs = [ pkgs.makeWrapper ];
                paths = [ _rust ];
                pathsToLink = [ "/" "/bin" ];
                postBuild = ''
                  for i in $out/bin/*; do
                    wrapProgram "$i" --prefix PATH : "$out/bin"
                  done
                '';
              };
            in
            pre-commit-hooks.lib.${system}.run {
              src = ./.;
              hooks = {
                rustfmt = {
                  enable = true;
                  entry = lib.mkForce "${rust}/bin/cargo-fmt fmt --all -- --config format_code_in_doc_comments=true --check --color always";
                };
                nixpkgs-fmt.enable = true;
                typos.enable = true;
                commitizen.enable = true; # conventional commits
                signedcommits = {
                  enable = true;
                  name = "signed-commits";
                  description = "Check whether the current commit message is signed";
                  stages = [ "push" ];
                  entry = "${signed-commits}/bin/signed-commits";
                  language = "system";
                  pass_filenames = false;
                };
              };
            };
        };

        devShells =
          let
            # pre-commit-checks
            _shellHook = (self.checks.${system}.pre-commit-check.shellHook or "");

            # devShells
            msrv = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ WASMInputs ++ [ msrv_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

            stable = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ WASMInputs ++ [ stable_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

            nightly = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ [ nightly_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

            coverage = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ [ coverage_toolchain pkgs.lcov ];
              inherit nativeBuildInputs;
              RUSTFLAGS = "-Cinstrument-coverage";
              RUSTDOCFLAGS = "-Cinstrument-coverage";
              LLVM_PROFILE_FILE = "./target/coverage/%p-%m.profraw";
            } // envVars);
          in
          {
            inherit msrv stable nightly coverage;
            default = msrv;
          };
      }
    );
}
