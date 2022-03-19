# Development Cycle

This project follows a regular releasing schedule similar to the one [used by the Rust language]. In short, this means that a new release is made at a regular cadence, with all the feature/bugfixes that made it to `master` in time. This ensures that we don't keep delaying releases waiting for "just one more little thing".

This project uses [Semantic Versioning], but is currently at MAJOR version zero (0.y.z) meaning it is still in initial development. Anything MAY change at any time. The public API SHOULD NOT be considered stable. Until we reach version `1.0.0` we will do our best to document any breaking API changes in the changelog info attached to each release tag.

We decided to maintain a faster release cycle while the library is still in "beta", i.e. before release `1.0.0`: since we are constantly adding new features and, even more importantly, fixing issues, we want developers to have access to those updates as fast as possible. For this reason we will make a release **every 4 weeks**.

Once the project reaches a more mature state (>= `1.0.0`), we will very likely switch to longer release cycles of **6 weeks**.

The "feature freeze" will happen **one week before the release date**. This means a new branch will be created originating from the `master` tip at that time, and in that branch we will stop adding new features and only focus on ensuring the ones we've added are working properly.

To create a new release a release manager will create a new issue using the `Release` template and follow the template instructions.

[used by the Rust language]: https://doc.rust-lang.org/book/appendix-07-nightly-rust.html
[Semantic Versioning]: https://semver.org/
