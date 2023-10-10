{
  description = "BDK Flake to run all tests locally and in CI";

  inputs = {
    # stable nixpkgs
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-23.05";
    # pin bitcoind to a specific version
    # find instructions here:
    # <https://lazamar.co.uk/nix-versions>
    # pinned to 0.25.0
    nixpkgs-bitcoind.url = "github:nixos/nixpkgs?rev=9957cd48326fe8dbd52fdc50dd2502307f188b0d";
    # Blockstream's esplora
    # inspired by fedimint CI
    nixpkgs-kitman.url = "github:jkitman/nixpkgs?rev=61ccef8bc0a010a21ccdeb10a92220a47d8149ac";
    crane = {
      url = "github:ipetkov/crane?rev=b7db46f0f1751f7b1d1911f6be7daf568ad5bc65";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        flake-utils.follows = "flake-utils";
      };
    };
    flake-utils.url = "github:numtide/flake-utils";
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
  };

  outputs = { self, nixpkgs, nixpkgs-bitcoind, nixpkgs-kitman, crane, rust-overlay, flake-utils, advisory-db, pre-commit-hooks, ... }:
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

        pkgs = import nixpkgs {
          inherit system overlays;
        };
        pkgs-bitcoind = import nixpkgs-bitcoind {
          inherit system overlays;
        };
        pkgs-kitman = import nixpkgs-kitman {
          inherit system;
        };

        # Signed Commits
        signed-commits = pkgs.writeShellApplication {
          name = "signed-commits";
          runtimeInputs = [ pkgs.git ];
          text = builtins.readFile ./ci/commits_verify_signature.sh;
        };

        # Toolchains
        # latest stable
        rustTarget = pkgs.rust-bin.stable.latest.default;
        # we pin clippy instead of using "stable" so that our CI doesn't break
        # at each new cargo release
        rustClippyTarget = pkgs.rust-bin.stable."1.67.0".default;
        # MSRV
        rustMSRVTarget = pkgs.rust-bin.stable."1.57.0".default;
        # WASM
        rustWASMTarget = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ];
        };
        # LLVM code coverage
        rustLLVMTarget = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "llvm-tools" ];
        };
        # Nighly Docs
        rustNightlyTarget = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);

        # Rust configs
        craneLib = (crane.mkLib pkgs).overrideToolchain rustTarget;
        # Clippy specific configs
        craneClippyLib = (crane.mkLib pkgs).overrideToolchain rustClippyTarget;
        # MSRV specific configs
        # WASM specific configs
        # craneUtils needs to be built using Rust latest (not MSRV/WASM)
        # check https://github.com/ipetkov/crane/issues/422
        craneMSRVLib = ((crane.mkLib pkgs).overrideToolchain rustMSRVTarget).overrideScope' (final: prev: { inherit (craneLib) craneUtils; });
        craneWASMLib = ((crane.mkLib pkgs).overrideToolchain rustWASMTarget).overrideScope' (final: prev: { inherit (craneLib) craneUtils; });
        # LLVM code coverage
        craneLLVMLib = (crane.mkLib pkgs).overrideToolchain rustLLVMTarget;
        craneNightlyLib = (crane.mkLib pkgs).overrideToolchain rustNightlyTarget;

        # Common inputs for all derivations
        buildInputs = [
          # Add additional build inputs here
          pkgs-bitcoind.bitcoind
          pkgs-kitman.esplora
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

        # Common derivation arguments used for all builds
        commonArgs = {
          # When filtering sources, we want to allow assets other than .rs files
          src = lib.cleanSourceWith {
            src = ./.; # The original, unfiltered source
            filter = path: type:
              # esplora uses `.md` in the source code
              (lib.hasSuffix "\.md" path) ||
              # bitcoind_rpc uses `.db` in the source code
              (lib.hasSuffix "\.db" path) ||
              # Default filter from crane (allow .rs files)
              (craneLib.filterCargoSources path type);
          };

          # Fixing name/version here to avoid warnings
          # This does not interact with the versioning
          # in any of bdk crates' Cargo.toml
          pname = "crates";
          version = "0.1.0";

          inherit buildInputs;
          inherit nativeBuildInputs;
          # Additional environment variables can be set directly
          BITCOIND_EXEC = "${pkgs.bitcoind}/bin/bitcoind";
          ELECTRS_EXEC = "${pkgs-kitman.esplora}/bin/esplora";
        };

        # MSRV derivation arguments
        MSRVArgs = {
          cargoLock = ./CargoMSRV.lock;
        };

        # WASM derivation arguments
        WASMArgs = {
          CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
          buildInputs = buildInputs ++ WASMInputs;
          inherit nativeBuildInputs;
          # crane tries to run the WASM file as if it were a binary
          doCheck = false;
          # just build bdk for now
          cargoExtraArgs = "--locked --target wasm32-unknown-unknown -p bdk --no-default-features --features bitcoin/no-std,miniscript/no-std,bdk_chain/hashbrown,dev-getrandom-wasm";
          # env vars
          CC = "${stdenv.cc.nativePrefix}cc";
          AR = "${stdenv.cc.nativePrefix}ar";
          CC_wasm32_unknown_unknown = "${pkgs.llvmPackages_14.clang-unwrapped}/bin/clang-14";
          CFLAGS_wasm32_unknown_unknown = "-I ${pkgs.llvmPackages_14.libclang.lib}/lib/clang/14.0.6/include/";
          AR_wasm32_unknown_unknown = "${pkgs.llvmPackages_14.llvm}/bin/llvm-ar";
        };

        buildDepsArgs = {
          cargoBuildCommand = "cargo build --profile ci";
        };


        # Caching: build *just* cargo dependencies for all crates, so we can reuse
        # all of that work (e.g. via cachix) when running in CI
        # all artifacts from running cargo {build,check,test} will be cached
        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // buildDepsArgs);
        cargoArtifactsMSRV = craneMSRVLib.buildDepsOnly (commonArgs // buildDepsArgs // MSRVArgs);
        cargoArtifactsWASM = craneWASMLib.buildDepsOnly (commonArgs // buildDepsArgs // WASMArgs);
        cargoArtifactsClippy = craneClippyLib.buildDepsOnly (commonArgs // buildDepsArgs);

        # Run clippy on the workspace source,
        # reusing the dependency artifacts (e.g. from build scripts or
        # proc-macros) from above.
        clippy = craneClippyLib.cargoClippy (commonArgs // {
          cargoArtifacts = cargoArtifactsClippy;
          cargoClippyExtraArgs = "--all-features --all-targets -- -D warnings";
        });

        # fmt
        fmt = craneLib.cargoFmt (commonArgs // {
          inherit cargoArtifacts;
          cargoExtraArgs = "--all";
          rustFmtExtraArgs = "--config format_code_in_doc_comments=true";
        });
      in
      rec {
        checks = {
          inherit clippy;
          inherit fmt;

          # Latest
          latest = packages.default;
          latestAll = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--all-features -- --test-threads=2"; # bdk_bitcoind_rpc test spams bitcoind
          });
          latestNoDefault = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--no-default-features -- --test-threads=2"; # bdk_bitcoind_rpc test spams bitcoind
          });
          latestNoStdBdk = craneLib.cargoBuild (commonArgs // {
            inherit cargoArtifacts;
            cargoCheckExtraArgs = "-p bdk --no-default-features --features bitcoin/no-std,miniscript/no-std,bdk_chain/hashbrown";
          });
          latestNoStdChain = craneLib.cargoBuild (commonArgs // {
            inherit cargoArtifacts;
            cargoCheckExtraArgs = "-p bdk_chain --no-default-features --features bitcoin/no-std,miniscript/no-std,hashbrown";
          });
          latestNoStdEsplora = craneLib.cargoBuild (commonArgs // {
            inherit cargoArtifacts;
            cargoCheckExtraArgs = "-p bdk_esplora --no-default-features --features bitcoin/no-std,miniscript/no-std,bdk_chain/hashbrown";
          });

          # MSRV
          MSRV = packages.MSRV;
          MSRVAll = craneMSRVLib.cargoTest (commonArgs // MSRVArgs // {
            cargoArtifacts = cargoArtifactsMSRV;
            cargoTestExtraArgs = "--all-features -- --test-threads=2"; # bdk_bitcoind_rpc test spams bitcoind
          });
          MSRVNoDefault = craneMSRVLib.cargoTest (commonArgs // MSRVArgs // {
            cargoArtifacts = cargoArtifactsMSRV;
            cargoTestExtraArgs = "--no-default-features -- --test-threads=2"; # bdk_bitcoind_rpc test spams bitcoind
          });
          MSRVNoStdBdk = craneMSRVLib.cargoBuild (commonArgs // MSRVArgs // {
            cargoArtifacts = cargoArtifactsMSRV;
            cargoCheckExtraArgs = "-p bdk --no-default-features --features bitcoin/no-std,miniscript/no-std,bdk_chain/hashbrown";
          });
          MSRVNoStdChain = craneMSRVLib.cargoBuild (commonArgs // MSRVArgs // {
            cargoArtifacts = cargoArtifactsMSRV;
            cargoCheckExtraArgs = "-p bdk_chain --no-default-features --features bitcoin/no-std,miniscript/no-std,hashbrown";
          });
          MSRVNoStdEsplora = craneMSRVLib.cargoBuild (commonArgs // MSRVArgs // {
            cargoArtifacts = cargoArtifactsMSRV;
            cargoCheckExtraArgs = "-p bdk_esplora --no-default-features --features bitcoin/no-std,miniscript/no-std,bdk_chain/hashbrown";
          });

          # WASM
          WASMBdk = craneWASMLib.buildPackage (commonArgs // WASMArgs // {
            cargoArtifacts = cargoArtifactsWASM;
          });
          WASMEsplora = craneWASMLib.cargoBuild (commonArgs // WASMArgs // {
            cargoArtifacts = cargoArtifactsWASM;
            cargoExtraArgs = "--locked -p bdk_esplora --no-default-features --features bitcoin/no-std,miniscript/no-std,bdk_chain/hashbrown,async";
          });

          # Audit dependencies
          audit = craneLib.cargoAudit (commonArgs // {
            inherit advisory-db;
          });

          # Pre-commit checks
          pre-commit-check = pre-commit-hooks.lib.${system}.run {
            src = ./.;
            hooks = {
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

        packages = {
          # Building: does a cargo build
          default = craneLib.cargoBuild (commonArgs // {
            inherit cargoArtifacts;
          });
          MSRV = craneMSRVLib.cargoBuild (commonArgs // MSRVArgs // {
            cargoArtifacts = cargoArtifactsMSRV;
          });
          WASM = craneWASMLib.cargoBuild (commonArgs // WASMArgs // {
            cargoArtifacts = cargoArtifactsWASM;
          });
        };
        legacyPackages = {
          ci = {
            pre-commit-check = checks.pre-commit-check;
            clippy = checks.clippy;
            fmt = checks.fmt;
            audit = checks.audit;
            latest = {
              all = checks.latestAll;
              noDefault = checks.latestNoDefault;
              noStdBdk = checks.latestNoStdBdk;
              noStdChain = checks.latestNoStdChain;
              noStdEsplora = checks.latestNoStdEsplora;
            };
            MSRV = {
              all = checks.MSRVAll;
              noDefault = checks.MSRVNoDefault;
              noStdBdk = checks.MSRVNoStdBdk;
              noStdChain = checks.MSRVNoStdChain;
              noStdEsplora = checks.MSRVNoStdEsplora;
            };
            WASM = {
              bdk = checks.WASMBdk;
              esplora = checks.WASMEsplora;
            };
            codeCoverage = craneLLVMLib.cargoLlvmCov (commonArgs // {
              inherit cargoArtifacts;
              cargoLlvmCovExtraArgs = "--ignore-filename-regex /nix/store --all-features --workspace --lcov --output-path $out -- --test-threads=2";
            });
          };
        };

        devShells = {
          default = craneLib.devShell {
            inherit cargoArtifacts;
            # inherit check build inputs
            checks = {
              clippy = checks.clippy;
              fmt = checks.fmt;
              default = checks.latest;
              all = checks.latestAll;
              noDefault = checks.latestNoDefault;
              noStdBdk = checks.latestNoStdBdk;
              noStdChain = checks.latestNoStdChain;
              noStdEsplora = checks.latestNoStdEsplora;
            };
            # dependencies
            packages = buildInputs ++ [
              pkgs.bashInteractive
              pkgs.git
              pkgs.ripgrep
              rustTarget
            ];
            # pre-commit-checks
            inherit (self.checks.${system}.pre-commit-check) shellHook;

            # env vars
            BITCOIND_EXEC = commonArgs.BITCOIND_EXEC;
            ELECTRS_EXEC = commonArgs.ELECTRS_EXEC;
          };
          MSRV = craneMSRVLib.devShell {
            cargoArtifacts = cargoArtifactsMSRV;
            # inherit check build inputs
            checks = {
              clippy = checks.clippy;
              fmt = checks.fmt;
              audit = checks.audit;
              default = checks.MSRV;
              all = checks.MSRVAll;
              noDefault = checks.MSRVNoDefault;
              noStdBdk = checks.MSRVNoStdBdk;
              noStdChain = checks.MSRVNoStdChain;
              noStdEsplora = checks.MSRVNoStdEsplora;
            };
            # dependencies
            packages = buildInputs ++ [
              pkgs.bashInteractive
              pkgs.git
              pkgs.ripgrep
              rustMSRVTarget
            ];

            # pre-commit-checks
            inherit (self.checks.${system}.pre-commit-check) shellHook;

            # env vars
            BITCOIND_EXEC = commonArgs.BITCOIND_EXEC;
            ELECTRS_EXEC = commonArgs.ELECTRS_EXEC;
          };
          WASM = craneWASMLib.devShell {
            # inherit check build inputs
            checks = {
              inherit (checks) clippy fmt audit MSRV MSRVAll MSRVNoDefault MSRVNoStdBdk MSRVNoStdChain MSRVNoStdEsplora;
            };
            # dependencies
            packages = buildInputs ++ WASMInputs ++ [
              pkgs.bashInteractive
              pkgs.git
              pkgs.ripgrep
              rustWASMTarget
            ];

            # pre-commit-checks
            inherit (self.checks.${system}.pre-commit-check) shellHook;

            # env vars
            BITCOIND_EXEC = commonArgs.BITCOIND_EXEC;
            ELECTRS_EXEC = commonArgs.ELECTRS_EXEC;
            CARGO_BUILD_TARGET = WASMArgs.CARGO_BUILD_TARGET;
            CC = WASMArgs.CC;
            AR = WASMArgs.AR;
            CC_wasm32_unknown_unknown = WASMArgs.CC_wasm32_unknown_unknown;
            CFLAGS_wasm32_unknown_unknown = WASMArgs.CFLAGS_wasm32_unknown_unknown;
            AR_wasm32_unknown_unknown = WASMArgs.AR_wasm32_unknown_unknown;
          };
          lcov = pkgs.mkShell {
            buildInputs = [ pkgs.lcov ];
          };
          docsNightly = craneNightlyLib.devShell {
            packages = buildInputs ++ [ rustNightlyTarget ];
            RUSTDOCFLAGS = "--cfg docsrs -Dwarnings";
            BITCOIND_EXEC = commonArgs.BITCOIND_EXEC;
            ELECTRS_EXEC = commonArgs.ELECTRS_EXEC;
          };
        };
      }
    );
}

          
