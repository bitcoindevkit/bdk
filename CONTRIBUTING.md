Contributing to BDK
==============================

The BDK project operates an open contributor model where anyone is welcome to
contribute towards development in the form of peer review, documentation,
testing and patches.

Anyone is invited to contribute without regard to technical experience,
"expertise", OSS experience, age, or other concern. However, the development of
cryptocurrencies demands a high-level of rigor, adversarial thinking, thorough
testing and risk-minimization.
Any bug may cost users real money. That being said, we deeply welcome people
contributing for the first time to an open source project or picking up Rust while
contributing. Don't be shy, you'll learn.

Communications Channels
-----------------------

Communication about BDK happens primarily on the [BDK Discord](https://discord.gg/dstn4dQ).

Discussion about code base improvements happens in GitHub [issues](https://github.com/bitcoindevkit/bdk/issues) and
on [pull requests](https://github.com/bitcoindevkit/bdk/pulls).

Contribution Workflow
---------------------

The codebase is maintained using the "contributor workflow" where everyone
without exception contributes patch proposals using "pull requests". This
facilitates social contribution, easy testing and peer review.

To contribute a patch, the workflow is as follows:

  1. Fork Repository
  2. Create topic branch
  3. Commit patches

In general commits should be atomic and diffs should be easy to read.
For this reason do not mix any formatting fixes or code moves with actual code
changes. Further, each commit, individually, should compile and pass tests, in
order to ensure git bisect and other automated tools function properly.

When adding a new feature, thought must be given to the long term technical
debt.
Every new feature should be covered by functional tests where possible.

When refactoring, structure your PR to make it easy to review and don't
hesitate to split it into multiple small, focused PRs.

The Minimum Supported Rust Version is **1.63.0** (enforced by our CI).

Commits should cover both the issue fixed and the solution's rationale.
These [guidelines](https://chris.beams.io/posts/git-commit/) should be kept in mind. Commit messages follow the ["Conventional Commits 1.0.0"](https://www.conventionalcommits.org/en/v1.0.0/) to make commit histories easier to read by humans and automated tools. All commits must be [GPG signed](https://docs.github.com/en/authentication/managing-commit-signature-verification/signing-commits).

To facilitate communication with other contributors, the project is making use
of GitHub's "assignee" field. First check that no one is assigned and then
comment suggesting that you're working on it. If someone is already assigned,
don't hesitate to ask if the assigned party or previous commenter are still
working on it if it has been a while.

Deprecation policy
------------------

Where possible, breaking existing APIs should be avoided. Instead, add new APIs and
use [`#[deprecated]`](https://github.com/rust-lang/rfcs/blob/master/text/1270-deprecation.md)
to discourage use of the old one.

Deprecated APIs are typically maintained for one release cycle. In other words, an
API that has been deprecated with the 0.10 release can be expected to be removed in the
0.11 release. This allows for smoother upgrades without incurring too much technical
debt inside this library.

If you deprecated an API as part of a contribution, we encourage you to "own" that API
and send a follow-up to remove it as part of the next release cycle.

Peer review
-----------

Anyone may participate in peer review which is expressed by comments in the
pull request. Typically reviewers will review the code for obvious errors, as
well as test out the patch set and opine on the technical merits of the patch.
PR should be reviewed first on the conceptual level before focusing on code
style or grammar fixes.

To merge a PR we require all CI tests to pass, the PR has at least one approving review by a maintainer with write access, and reasonable criticisms have been addressed.

Coding Conventions
------------------

This codebase uses spaces, not tabs.
Run `just check` to check formatting, linting, compilation and commit signing, `just fmt` to format code before commiting, and `just test` to run tests for all crates.
This is also enforced by the CI.
All public items must be documented. We adhere to the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/about.html) with respect to documentation.

The library is written using safe rust. Special consideration must be given to code which proposes an exception to the rule.

All new features require testing. Tests should be unique and self-describing. If a test is in development or is broken or no longer useful, then a reason should be given for adding the `#[ignore]` attribute.

Security
--------

Security is a high priority of BDK; disclosure of security vulnerabilities helps
prevent user loss of funds.

Note that BDK is currently considered "pre-production" during this time, there
is no special handling of security issues. Please simply open an issue on
Github.

BDK requires all commits to be signed using PGP. Refer to
[this guide](https://git-scm.com/book/en/v2/Git-Tools-Signing-Your-Work)
if you don't have a PGP key set up with `git` yet.

Testing
-------

Related to the security aspect, BDK developers take testing very seriously.
Due to the modular nature of the project, writing new functional tests is easy
and good test coverage of the codebase is an important goal.
Refactoring the project to enable fine-grained unit testing is also an ongoing
effort.

First Time Contributors
-----------------------

If it is your first time contributing to the BDK family of libraries, welcome! We're glad to have you with us. If your
first (or few first) PRs are focused on very small fixes to documentation, however, they might not meet our threshold
for acceptance for first time contributors.

Minor grammar and punctuation fixes aren't a good way to start contributing to a project, and instead we suggest you
start with something a little more substantial. It's better to find an issue where you can demonstrate some knowledge
of bitcoin or the code base, such as improving the substance of documentation, testing, or fixing some small issue
even if it's considered low priority.

This being said we are always looking forward to working with new folks interested in contributing to our libraries.
If you are looking for issues to work on, check out the good first issue label and join our Discord server!

Going further
-------------

You may be interested by Jon Atacks guide on [How to review Bitcoin Core PRs](https://github.com/jonatack/bitcoin-development/blob/master/how-to-review-bitcoin-core-prs.md)
and [How to make Bitcoin Core PRs](https://github.com/jonatack/bitcoin-development/blob/master/how-to-make-bitcoin-core-prs.md).
While there are differences between the projects in terms of context and
maturity, many of the suggestions offered apply to this project.

Overall, have fun :)
