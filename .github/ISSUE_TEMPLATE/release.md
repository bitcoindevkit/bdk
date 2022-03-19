---
name: Release
about: Create a new release [for release managers only]
title: 'Release MAJOR.MINOR.PATCH'
labels: 'release'
assignees: ''

---

Create a new release.

**Summary**

<--release summary to be used in announcements-->

**Commit**

<--latest commit ID to include in this release-->

**Changelog**

<--add notices from PRs merged since the prior release, see ["keep a changelog"]-->

**Checklist**  

Release numbering must follow [Semantic Versioning]. These steps assume the
current `master` branch **development** version is **MAJOR.MINOR.0**.

On the day of the feature freeze for the current development **MAJOR.MINOR.0**
release, create a new release branch and release candidate tag:

- [ ] Double check that your local `master` is up-to-date with the upstream repo.
- [ ] Create a new branch called `release/MAJOR.MINOR` from `master`.
- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR` branch.
  - The tag name should be `vMAJOR.MINOR.0-RC.1`
  - Use message "Release MAJOR.MINOR.0 RC.1".  
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Push the `release/MAJOR.MINOR` branch and new tag to the `bitcoindevkit/bdk` repo.
  - Use `git push --tags` option to push the new `vMAJOR.MINOR.0-RC.1` tag.

Change `master` branch version to next development **MAJOR.MINOR+1.0** release:

- [ ] Switch back to the `master` branch.
- [ ] Create a new PR branch called `bump_dev_MAJOR_MINOR+1`, eg. `bump_dev_0_18`.
- [ ] Bump the `bump_dev_MAJOR_MINOR+1` branch to the next development MAJOR.MINOR+1 version.
  - Change the `Cargo.toml` version value to `MAJOR.MINOR+1.0`.
  - The commit message should be "Bump version to MAJOR.MINOR+1.0".
- [ ] Create PR and merge the `bump_dev_MAJOR_MINOR+1` branch to `master`.
  - Title PR "Bump dev version to MAJOR.MINOR+1.0".

If any issues need to be fixed before the **MAJOR.MINOR.0** version is released:

- [ ] Merge fix PRs to the `master` branch.
- [ ] Git cherry-pick fix commits to the `release/MAJOR.MINOR` branch.
- [ ] Verify fixes in `release/MAJOR.MINOR` branch.
- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR` branch.
  - The tag name should be `vMAJOR.MINOR.0-RC.x+1`, where x is the current release candidate number.
  - Use tag message "Release MAJOR.MINOR.0 RC.x+1".  
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Push the new tag to the `bitcoindevkit/bdk` repo.
  - Use `git push --tags` option to push the new `vMAJOR.MINOR.0-RC.x+1` tag.

On the day of a **MAJOR.MINOR.0** release:

- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR` branch.
  - The tag name should be `vMAJOR.MINOR.0`
  - The first line of the tag message should be "Release MAJOR.MINOR.0".
  - In the body of the tag message put a copy of the **Summary** and **Changelog** for the release.
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Wait for the CI to finish one last time.
- [ ] Push the new tag to the `bitcoindevkit/bdk` repo.
- [ ] Publish **all** the updated crates to crates.io.
- [ ] Create the release on GitHub.
  - Go to "tags", click on the dots on the right and select "Create Release".
  - Set the title to `Release MAJOR.MINOR.0`.
  - In the release notes body put the **Summary** and **Changelog**.
  - Use the "+ Auto-generate release notes" button to add details from included PRs.
  - Until we reach a `1.0.0` release check the "Pre-release" box.
- [ ] Make sure the new release shows up on [crates.io] and that the docs are built correctly on [docs.rs].
- [ ] Announce the release, using the **Summary**, on Discord, Twitter and Mastodon.
- [ ] Celebrate :tada:

On the day of a **MAJOR.MINOR.PATCH+1** release:

- [ ] Merge fix PRs to the `master` branch.
- [ ] Git cherry-pick fix commits to the `release/MAJOR.MINOR` branch to be patched.
- [ ] Verify fixes in `release/MAJOR.MINOR` branch.
- [ ] Add a tag to the `HEAD` commit in the `release/MAJOR.MINOR` branch.
  - The tag name should be `vMAJOR.MINOR.PATCH+1`
  - The first line of the tag message should be "Release MAJOR.MINOR.PATCH+1".
  - In the body of the tag message put a copy of the **Summary** and **Changelog** for the release.
  - Make sure the tag is signed, for extra safety use the explicit `--sign` flag.
- [ ] Wait for the CI to finish one last time.
- [ ] Push the new tag to the `bitcoindevkit/bdk` repo.
- [ ] Publish **all** the updated crates to crates.io.
- [ ] Create the release on GitHub.
  - Go to "tags", click on the dots on the right and select "Create Release".
  - Set the title to `Release MAJOR.MINOR.PATCH+1`.
  - In the release notes body put the **Summary** and **Changelog**.
  - Use the "+ Auto-generate release notes" button to add details from included PRs.
  - Until we reach a `1.0.0` release check the "Pre-release" box.
- [ ] Make sure the new release shows up on [crates.io] and that the docs are built correctly on [docs.rs].
- [ ] Announce the release, using the **Summary**, on Discord, Twitter and Mastodon.
- [ ] Celebrate :tada:

[Semantic Versioning]: https://semver.org/
[crates.io]: https://crates.io/crates/bdk
[docs.rs]: https://docs.rs/bdk/latest/bdk
["keep a changelog"]: https://keepachangelog.com/en/1.0.0/
