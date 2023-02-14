---
name: Minor Release
about: Create a new minor release [for release managers only]
title: 'Release MAJOR.MINOR+1.0'
labels: 'release'
assignees: ''

---

## Create a new minor release

### Summary

<--release summary to be used in announcements-->

### Commit

<--latest commit ID to include in this release-->

### Changelog

<--add notices from PRs merged since the prior release, see ["keep a changelog"]-->

### Checklist

Release numbering must follow [Semantic Versioning]. These steps assume the current `master`
branch **development** version is *MAJOR.MINOR.0*.

#### On the day of the feature freeze

Change the `master` branch to the next MINOR+1 version:

- [ ] Switch to the `master` branch.
- [ ] Create a new PR branch called `bump_dev_MAJOR_MINOR+1`, eg. `bump_dev_0_22`.
- [ ] Bump the `bump_dev_MAJOR_MINOR+1` branch to the next development MINOR+1 version.
  - Change the `Cargo.toml` version value to `MAJOR.MINOR+1.0`.
  - Update the `CHANGELOG.md` file.
  - The commit message should be "Bump version to MAJOR.MINOR+1.0".
- [ ] Create PR and merge the `bump_dev_MAJOR_MINOR+1` branch to `master`.
  - Title PR "Bump version to MAJOR.MINOR+1.0".

Create a new release branch and release candidate tag:

- [ ] Double check that your local `master` is up-to-date with the upstream repo.
- [ ] Create a new branch called `release/MAJOR.MINOR+1` from `master`.
- [ ] Bump the `release/MAJOR.MINOR+1` branch to `MAJOR.MINOR+1.0-rc.1` version.
  - Change the `Cargo.toml` version value to `MAJOR.MINOR+1.0-rc.1`.
  - The commit message should be "Bump version to MAJOR.MINOR+1.0-rc.1".
- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR+1` branch.
  - The tag name should be `vMAJOR.MINOR+1.0-rc.1`
  - Use message "Release MAJOR.MINOR+1.0 rc.1".
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Push the `release/MAJOR.MINOR` branch and new tag to the `bitcoindevkit/bdk` repo.
  - Use `git push --tags` option to push the new `vMAJOR.MINOR+1.0-rc.1` tag.

If any issues need to be fixed before the *MAJOR.MINOR+1.0* version is released:

- [ ] Merge fix PRs to the `master` branch.
- [ ] Git cherry-pick fix commits to the `release/MAJOR.MINOR+1` branch.
- [ ] Verify fixes in `release/MAJOR.MINOR+1` branch.
- [ ] Bump the `release/MAJOR.MINOR+1` branch to `MAJOR.MINOR+1.0-rc.x+1` version.
  - Change the `Cargo.toml` version value to `MAJOR.MINOR+1.0-rc.x+1`.
  - The commit message should be "Bump version to MAJOR.MINOR+1.0-rc.x+1".
- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR+1` branch.
  - The tag name should be `vMAJOR.MINOR+1.0-rc.x+1`, where x is the current release candidate number.
  - Use tag message "Release MAJOR.MINOR+1.0 rc.x+1".
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Push the new tag to the `bitcoindevkit/bdk` repo.
  - Use `git push --tags` option to push the new `vMAJOR.MINOR+1.0-rc.x+1` tag.

#### On the day of the release

Tag and publish new release:

- [ ] Bump the `release/MAJOR.MINOR+1` branch to `MAJOR.MINOR+1.0` version.
  - Change the `Cargo.toml` version value to `MAJOR.MINOR+1.0`.
  - The commit message should be "Bump version to MAJOR.MINOR+1.0".
- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR+1` branch.
  - The tag name should be `vMAJOR.MINOR+1.0`
  - The first line of the tag message should be "Release MAJOR.MINOR+1.0".
  - In the body of the tag message put a copy of the **Summary** and **Changelog** for the release.
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Wait for the CI to finish one last time.
- [ ] Push the new tag to the `bitcoindevkit/bdk` repo.
- [ ] Publish **all** the updated crates to crates.io.
- [ ] Create the release on GitHub.
  - Go to "tags", click on the dots on the right and select "Create Release".
  - Set the title to `Release MAJOR.MINOR+1.0`.
  - In the release notes body put the **Summary** and **Changelog**.
  - Use the "+ Auto-generate release notes" button to add details from included PRs.
  - Until we reach a `1.0.0` release check the "Pre-release" box.
- [ ] Make sure the new release shows up on [crates.io] and that the docs are built correctly on [docs.rs].
- [ ] Announce the release, using the **Summary**, on Discord, Twitter and Mastodon.
- [ ] Celebrate 🎉

[Semantic Versioning]: https://semver.org/
[crates.io]: https://crates.io/crates/bdk
[docs.rs]: https://docs.rs/bdk/latest/bdk
["keep a changelog"]: https://keepachangelog.com/en/1.0.0/
