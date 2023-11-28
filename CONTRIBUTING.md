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

Communication Channels
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
  1. Create topic branch
  1. Commit patches

In general commits should be atomic and diffs should be easy to read.
For this reason do not mix any formatting fixes or code moves with actual code
changes. Further, each commit, individually, should compile and pass tests, in
order to ensure git bisect and other automated tools function properly.

When adding a new feature, thought must be given to the long term technical
debt.
Every new feature should be covered by functional tests where possible.

When refactoring, structure your PR to make it easy to review and don't
hesitate to split it into multiple small, focused PRs.

The Minimal Supported Rust Version is **1.57.0** (enforced by our CI).

To facilitate communication with other contributors, the project is making use
of GitHub's "assignee" field. First check that no one is assigned and then
comment suggesting that you're working on it. If someone is already assigned,
don't hesitate to ask if the assigned party or previous commenter are still
working on it if it has been awhile.

Our CI can be replicated locally with Nix.
Just run `nix flake check`.
Our Nix setup can greatly increase developer experience
and onboarding of new contributors.
Please check the [`NIX.md`](NIX.md) for details
on how to install Nix and why we use it.

Commit policy
-------------

Commits should cover both the issue fixed and the solution's rationale.
These [guidelines](https://chris.beams.io/posts/git-commit/) should be kept in mind.

We enforce two commit styles in our CI:

1. Commits should be signed with GPG using a key with a valid email address.
1. Commit messages should follow the ["Conventional Commits 1.0.0"](https://www.conventionalcommits.org/en/v1.0.0/)
   to make commit histories easier to read by humans and automated tools.
   Commits starting with `Merge ...` which GitHub automatically generates
   are exempt from this rule.

You can use Nix to automatically and easily check locally these rules before commit and push events.
Please check the [`NIX.md`](NIX.md) for details.

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

Coding Conventions
------------------

This codebase uses spaces, not tabs.
Use `cargo fmt` with the default settings to format code before committing.
This is also enforced by the CI.

Security
--------

Security is a high priority of BDK; disclosure of security vulnerabilities helps
prevent user loss of funds.

Note that BDK is currently considered "pre-production" during this time, there
is no special handling of security issues. Please simply open an issue on
Github.

Testing
-------

Related to the security aspect, BDK developers take testing very seriously.
Due to the modular nature of the project, writing new functional tests is easy
and good test coverage of the codebase is an important goal.
Refactoring the project to enable fine-grained unit testing is also an ongoing
effort.

Going further
-------------

You may be interested by Jon Atacks guide on [How to review Bitcoin Core PRs](https://github.com/jonatack/bitcoin-development/blob/master/how-to-review-bitcoin-core-prs.md)
and [How to make Bitcoin Core PRs](https://github.com/jonatack/bitcoin-development/blob/master/how-to-make-bitcoin-core-prs.md).
While there are differences between the projects in terms of context and
maturity, many of the suggestions offered apply to this project.

Overall, have fun :)
