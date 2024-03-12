{
  description = "BDK Flake to run all tests locally and in CI";

  inputs = {
    # stable nixpkgs (let's not YOLO on unstable)
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-23.11";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        flake-utils.follows = "flake-utils";
      };
    };

    flake-utils.url = "github:numtide/flake-utils";

    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, pre-commit-hooks, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        lib = pkgs.lib;
        stdenv = pkgs.stdenv;
        isDarwin = stdenv.isDarwin;
        libsDarwin = with pkgs.darwin.apple_sdk.frameworks; lib.optionals isDarwin [
          # Additional darwin specific inputs can be set here
          Security
          SystemConfiguration
          CoreServices
        ];

        # Dependencies
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        pkgs-bitcoind = pkgs.callPackage ./nix/packages/bitcoind.nix { };

        pkgs-esplora = pkgs.callPackage ./nix/packages/esplora/derivation.nix {
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
        stable = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };
        # MSRV stable
        msrv = pkgs.rust-bin.stable."1.63.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };
        # Nighly for docs
        nightly = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);
        # Code coverage
        coverage = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
          extensions = [ "llvm-tools-preview" ];
        };

        # Common inputs
        envVars = {
          BITCOIND_EXEC = "${pkgs-bitcoind}/bin/bitcoind";
          ELECTRS_EXEC = "${pkgs-esplora}/bin/esplora";
          CC = "${stdenv.cc.nativePrefix}cc";
          AR = "${stdenv.cc.nativePrefix}ar";
          CC_wasm32_unknown_unknown = "${pkgs.llvmPackages_14.clang-unwrapped}/bin/clang-14";
          CFLAGS_wasm32_unknown_unknown = "-I ${pkgs.llvmPackages_14.libclang.lib}/lib/clang/14.0.6/include/";
          AR_wasm32_unknown_unknown = "${pkgs.llvmPackages_14.llvm}/bin/llvm-ar";
        };
        buildInputs = [
          # Add additional build inputs here
          pkgs-bitcoind
          pkgs-esplora
          pkgs.openssl
          pkgs.openssl.dev
          pkgs.pkg-config
          pkgs.curl
          pkgs.libiconv
        ] ++ libsDarwin;

        # WASM deps
        WASMInputs = [
          # Additional wasm specific inputs can be set here
          pkgs.llvmPackages_14.clang-unwrapped
          pkgs.llvmPackages_14.stdenv
          pkgs.llvmPackages_14.libcxxClang
          pkgs.llvmPackages_14.libcxxStdenv
        ];

        nativeBuildInputs = [
          # Add additional build inputs here
          pkgs.python3
        ] ++ lib.optionals isDarwin [
          # Additional darwin specific native inputs can be set here
        ];
      in
      {
        packages = {
          bitcoind = pkgs-bitcoind;
          esplora = pkgs-esplora;
        };
        checks = {
          # Pre-commit checks
          pre-commit-check =
            let
              # this is a hack based on https://github.com/cachix/pre-commit-hooks.nix/issues/126
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
                # clippy = {
                #   enable = true;
                #   entry = lib.mkForce "${rust}/bin/cargo-clippy clippy --all-targets --all-features -- -D warnings";
                # };
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
          in
          {
            default = msrv;

            msrv = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ WASMInputs ++ [ msrv ];
              inherit nativeBuildInputs;
            } // envVars);

            stable = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ WASMInputs ++ [ stable ];
              inherit nativeBuildInputs;
            } // envVars);


            nightly = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ [ nightly ];
              inherit nativeBuildInputs;
            } // envVars);

            coverage = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ [ coverage pkgs.lcov ];
              inherit nativeBuildInputs;
              RUSTFLAGS = "-Cinstrument-coverage";
              RUSTDOCFLAGS = "-Cinstrument-coverage";
              LLVM_PROFILE_FILE = "./target/coverage/%p-%m.profraw";
            } // envVars);
          };
      }
    );
}
