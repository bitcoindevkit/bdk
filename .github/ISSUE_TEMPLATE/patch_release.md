---
name: Patch Release
about: Create a new patch release [for release managers only]
title: 'Release MAJOR.MINOR.PATCH+1'
labels: 'release'
assignees: ''

---

## Create a new patch release

### Summary

<--release summary to be used in announcements-->

### Commit

<--latest commit ID to include in this release-->

### Changelog

<--add notices from PRs merged since the prior release, see ["keep a changelog"]-->

### Checklist

Release numbering must follow [Semantic Versioning]. These steps assume the current `release/MAJOR.MINOR` branch version is *MAJOR.MINOR.PATCH*. The `master` branch stays on its *MAJOR.MINOR+1.0-alpha.0* development version and is not bumped for patch releases.

### On the day of the patch release

Cherry-pick, tag and publish new PATCH+1 release:

- [ ] Merge fix PRs to the `master` branch.
- [ ] Git cherry-pick fix commits to the `release/MAJOR.MINOR` branch to be patched.
- [ ] Verify fixes in `release/MAJOR.MINOR` branch.
- [ ] Bump the `release/MAJOR.MINOR.PATCH+1` branch to `MAJOR.MINOR.PATCH+1` version.
  - Change the `Cargo.toml` version value to `MAJOR.MINOR.MINOR.PATCH+1`.
  - Update the `CHANGELOG.md` file.
  - The commit message should be "Bump version to MAJOR.MINOR.PATCH+1".
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
- [ ] Celebrate 🎉

[Semantic Versioning]: https://semver.org/
[crates.io]: https://crates.io/crates/bdk
[docs.rs]: https://docs.rs/bdk/latest/bdk
["keep a changelog"]: https://keepachangelog.com/en/1.0.0/
