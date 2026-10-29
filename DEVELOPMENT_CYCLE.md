# Development Cycle

This project follows a regular releasing schedule similar to the one [used by the Rust language]. In short, this means that a new release is made at a regular cadence, with all the feature/bugfixes that made it to `master` in time. This ensures that we don't keep delaying releases waiting for "just one more little thing".

This project uses [Semantic Versioning], but is currently at MAJOR version zero (0.y.z) meaning it is still in initial development. Anything MAY change at any time. The public API SHOULD NOT be considered stable. Until we reach version `1.0.0` we will do our best to document any breaking API changes in the changelog info attached to each release.

We decided to maintain a faster release cycle while the library is still in "beta", i.e. before release `1.0.0`: since we are constantly adding new features and, even more importantly, fixing issues, we want developers to have access to those updates as fast as possible. For this reason we will make a release **every 4 weeks**.

Once the project reaches a more mature state (>= `1.0.0`), we will very likely switch to longer release cycles of **6 weeks**.

The "feature freeze" will happen **one week before the release date**. This means a new branch will be created originating from the `master` tip at that time, and in that branch we will stop adding new features and only focus on ensuring the ones we've added are working properly.

## Releasing

Releases are prepared and published with [release-plz] from the **Release-plz** GitHub Actions workflow. The workflow is triggered manually from the branch to release: `master` for regular releases or a `release/*` branch for patch and pre-releases.

1. Trigger the **Release-plz** workflow on the branch. The `release-pr` job opens or updates a release PR containing the version bumps and `CHANGELOG.md` changes for every workspace crate that has changes.
2. Review the release PR. Changelogs are generated from [Conventional Commits], so do not edit them by hand. Merge the PR once CI passes.
3. Trigger the **Release-plz** workflow again on the same branch. The `release` job publishes every unpublished crate to crates.io, pushes a `<crate>-<version>` tag for each crate, and creates the matching GitHub release. A reviewer must approve the `release` environment before publishing starts.
4. Verify the new versions on crates.io and docs.rs, then announce the release.

release-plz supports only one open release PR per repository, so do not prepare releases on two branches at the same time.

For a patch release, cherry-pick the fixes onto the appropriate `release/*` branch before triggering the workflow. Make sure the crate versions on the selected branch are not behind the versions already published on crates.io; release-plz uses crates.io as its baseline.

Before the `release` job can publish, [crates.io trusted publishing] must be configured for every workspace crate with workflow name `release-plz.yml` and environment `release`.

[release-plz]: https://release-plz.dev
[Conventional Commits]: https://www.conventionalcommits.org
[crates.io trusted publishing]: https://crates.io/docs/trusted-publishing

[used by the Rust language]: https://doc.rust-lang.org/book/appendix-07-nightly-rust.html
[Semantic Versioning]: https://semver.org/
