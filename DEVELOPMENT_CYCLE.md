# Development Cycle

This project follows a regular releasing schedule similar to the one [used by the Rust language](https://doc.rust-lang.org/book/appendix-07-nightly-rust.html). In short, this means that a new release is made at a regular cadence, with all the feature/bugfixes that made it to `master` in time. This ensures that we don't keep delaying releases waiting for "just one more little thing".

We decided to maintain a faster release cycle while the library is still in "beta", i.e. before release `1.0.0`: since we are constantly adding new features and, even more importantly, fixing issues, we want developers to have access to those updates as fast as possible. For this reason we will make a release **every 4 weeks**.

Once the project will have reached a more mature state (>= `1.0.0`), we will very likely switch to longer release cycles of **6 weeks**.

The "feature freeze" will happen **one week before the release date**. This means a new branch will be created originating from the `master` tip at that time, and in that branch we will stop adding new features and only focus on ensuring the ones we've added are working properly.

```
master:           - - - - * - - - * - - - - - - * - - - * ...
                          |      /              |       |
release/0.x.0:            * - - #               |       |
                                                |      /
release/0.y.0:                                  * - - #
```

As soon as the release is tagged and published, the `release` branch will be merged back into `master` to update the version in the `Cargo.toml` to apply the new `Cargo.toml` version and all the other fixes made during the feature freeze window.

## Making the Release

What follows are notes and procedures that maintaners can refer to when making releases. All the commits and tags must be signed and, ideally, also [timestamped](https://github.com/opentimestamps/opentimestamps-client/blob/master/doc/git-integration.md).

Pre-`v1.0.0` our "major" releases only affect the "minor" semver value. Accordingly, our "minor" releases will only affect the "patch" value.

1. Create a new branch called `release/x.y.z` from `master`. Double check that your local `master` is up-to-date with the upstream repo before doing so.
2. Make a commit on the release branch to bump the version to `x.y.z-rc.1`. The message should be "Bump version to x.y.z-rc.1".
3. Push the new branch to `bitcoindevkit/bdk` on GitHub.
4. During the one week of feature freeze run additional tests on the release branch.
5. If a bug is found:
    - If it's a minor issue you can just fix it in the release branch, since it will be merged back to `master` eventually
    - For bigger issues you can fix them on `master` and then *cherry-pick* the commit to the release branch
6. Update the changelog with the new release version.
7. Update `src/lib.rs` with the new version (line ~59)
8. On release day, make a commit on the release branch to bump the version to `x.y.z`. The message should be "Bump version to x.y.z".
9. Add a tag to this commit. The tag name should be `vx.y.z` (for example `v0.5.0`), and the message "Release x.y.z". Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
10. Push the new commits to the upstream release branch, wait for the CI to finish one last time.
11. Publish **all** the updated crates to crates.io.
12. Make a new commit to bump the version value to `x.y.(z+1)-dev`. The message should be "Bump version to x.y.(z+1)-dev".
13. Merge the release branch back into `master`.
14. Create the release on GitHub: go to "tags", click on the dots on the right and select "Create Release". Then set the title to `vx.y.z` and write down some brief release notes.
15. Make sure the new release shows up on crates.io and that the docs are built correctly on docs.rs.
16. Announce the release on Twitter, Discord and Telegram.
17. Celebrate :tada:
