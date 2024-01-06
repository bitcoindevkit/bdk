# BDK's Nix Guidelines

This document outlines:

1. What is Nix and how to install.
1. BDK's Nix-based tests and CI.
1. Developer best-practices with Nix.
1. How to run replicate CI tests locally.

## Nix

We use [Nix](https://nixos.org/) as the CI tool for BDK and also as a
development environment for the project.
Nix is purely functional.
Everything is described as an expression/function,
taking some inputs and producing deterministic outputs.
This guarantees reproducible results and makes caching everything easy.
Nix expressions are lazy. Anything described in Nix code will only be executed
if some other expression needs its results.
This is very powerful but somewhat unnatural for developers not familiar
with functional programming.

There are several resources to get started with Nix:

- [NixOS Wiki](https://nixos.wiki/).
- [Nix Reference Manual](https://nixos.org/manual/nix/stable/).
- [`nixpkgs` Manual](https://nixos.org/manual/nixpkgs/stable/).
- [Official documentation for getting things done with Nix](https://nix.dev/).

There's also the [`nix-bitcoin` project](https://nixbitcoin.org/).

To install Nix please follow the [official Nix documentation](https://nixos.org/manual/nix/stable/installation/installation.html).
You may need to enable Flakes support, check the instructions at [NixOS Wiki entry on Flakes](https://nixos.wiki/wiki/Flakes).

## Why are we using Nix

We want to make BDK's development easier.
This means two things:

1. Enhance developer experience.
1. Facilitate onboarding of new contributors.

Both of these can be accomplished with Nix.

BDK has many crates in the workspace,
and proper development and testing needs dependencies to be installed,
and environment variables to be set.
This can be daunting for new contributors.

BDK needs several Rust versions with different compilation targets.
This can be cumbersome to install, but also needs proper maintainability,
since these versions need to be updated frequently.

BDK enforces commit styles and commit policies (please check [CONTRIBUTING.md](CONTRIBUTING.md)).
This is difficult to enforce locally
and can be a source of frustration during contribution reviews,
potentially leading to a lot of wasted time and pushing new contributors away.

Finally, BDK has a lot of tests and checks.
It is difficult to replicate these locally.

All the above can easily be accomplished with Nix.
Nix is available for macOS, Linux, and Windows;
while also being easy to install.
Nix has a rich [community](https://nixos.org/community/) and one can find help
on the [Nix's Forums](https://discourse.nixos.org/),
[Nix's Discord](https://discord.gg/RbvHtGa),
and [Nix's Matrix channel](https://matrix.to/#/#community:nixos.org).
Additionally, Nix issues and errors can be searched in any search engine,
and in the [Nix's stackoverflow](https://stackoverflow.com/questions/tagged/nix+or+nixpkgs+or+nixos+or+nixops).

## BDK's Nix-based tests and CI

**TLDR: To replicate everything that our CI checks just do `nix flake check`**.

BDK's tests and checks are run using Nix.
If you want to run the tests locally, you can do so with:

```text
nix flake -L check --keep-failed
```

The `-L` flag prints full build logs on standard error.
This is good for debugging.
The `--keep-failed` flag keep temporary directories of failed builds.
This can be useful for caching intermediate results while fixing failed checks.

Under the hood `nix flake check` does the following:

- Checks for typos, GPG-signed and conventional commits, `rustfmt`, `nixpkgs-fmt` (`.nix` files).
- Runs `cargo clippy` in all workspace.
- Runs `cargo fmt` in all workspace.
- Checks dependencies for security advisory using [`rustsec/advisory-db`](https://github.com/rustsec/advisory-db).
- `cargo build` and `cargo test` in whole workspace using the latest Rust version.
- `cargo build` and `cargo test` in whole workspace using the MSRV Rust version.
- `cargo build` and `cargo check` WASM targets using the latest Rust version
   on `bdk` and `bdk_esplora` crates.

The checks are by default cached by Nix and subsequent runs use the cached results,
respecting the Nix derivations' dependency graph.

All the necessary dependencies, e.g. `bitcoind` and `esplora`,
along with all necessary environment variables,
are installed and available in the environment that Nix's checks runs in.

Individual checks from the above list can be listed with `nix flake show`
and will be displayed under the `checks` tree.
You can run an individual check with:

```text
nix build -L .#checks.SYSTEM.CHECK --keep-failed
```

Where `SYSTEM` is the system you are running on,
and `CHECK` is the name of the check you want to run.
For example, in a MacOS Apple Silicon machine (`aarch64-darwin`)
 and for the `clippy` check you can run:

```text
nix build -L .#checks.aarch64-darwin.clippy --keep-failed
```

We also group all the CI checks inside the `legacyPackages`.
These can be viewed in the `flake.nix` file under the `legacyPackages.ci` set.
You can run any of these with:

```text
nix build -L .#ci.CHECK --keep-failed
```

Where `CHECK` is the name of the check you want to run.
For example to run the `pre-commit-check`:

```text
nix build -L .#ci.pre-commit-check --keep-failed
```

You can check all the tests that our CI performs by inspecting both
the `flake.nix` and `.github/workflows/cont_integration.yml` files.

## Developer best-practices with Nix

We provide several development shells,
also called `devShell`s,
that assist developers with the necessary dependency and environment
to develop and test BDK.

To access these shells, you can run:

```text
nix develop .#SHELL
```

Where `SHELL` is the name of the `devShell` you want to use.
We provide the following shells:

- default: latest Rust, you can omit the `#SHELL` and just run `nix develop .`.
- `MSRV`: MSRV Rust version.
- `WASM`: latest Rust with WASM compilation support.

All of these `devShell`s handle all the necessary dependencies and environment variables needed.
They also provide pre-commit git hooks that checks either during commit or push events the following:

- GPG-signed commits (on a `git push` event).
- Conventional commit style commit messages (on a `git commit` event).
- `nixpkgs-fmt` adherence (on a `git commit` event).
- Typos in the commit content (on a `git commit` event).

Regarding the typos check, there might be some false positives.
In that case you can add a regex rule to filter out the typos in the `.typos.toml` file.
We already included all the false positives we've found so far with some explainable comments.
Hence, you should be able to follow the examples and add your own.
Additionally, you can find more information in [`crate-ci/typos`](https://github.com/crate-ci/typos).

As a final note on the `devShell`s, they can be conveniently automated with
[`nix-community/nix-direnv`](https://github/nix-community/nix-direnv).
`nix-direnv`, once installed, will:

- Enable shell completion in `devShell`s.
- Seamless integrate pre-commit checks even outside a `devShell`.
- Improved caching of `devShell`s.
