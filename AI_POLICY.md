# AI Policy

Using AI (i.e. LLMs) as tools for coding is welcome, provided that such use adheres to the following policy, adds value, and does not waste the time of other contributors.

A high bar is held for all contributions to this project.

Changes touching key derivation, signing, PSBT handling, or other consensus-critical
paths are held to a higher bar still: a subtle bug there can cost users real money, so
AI-assisted contributions in these areas need correspondingly thorough tests and a
clear explanation, in your own words, of why the change is correct.

**AI should not be used to generate comments when communicating with maintainers and other contributors**.
Comments are expected to be written by humans.
Comments that are believed to be written by AI may be moderated.

If you are opening an issue, you should be able to describe the problem in your own words.

You should only open a pull request if you (the human behind the LLM):

- know the language the code is written in
- could have written the code yourself
- understand the existing surrounding code and the effect of the suggested changes

You must also be able to explain the pull request changes in your own words.
This includes the pull request body and responses to questions.
**Do not copy responses from the AI when replying to questions from reviewers.**

This project requires a human author in the loop who understands the work produced by AI.
**Pull requests should not be opened or driven by autonomous agents**.
A human author must choose the work, understand the change, and be responsible for the contribution.
Do not include agents as authors or co-authors of your commits for these reasons.
Pull requests that appear in violation of this can be closed without notice.

If you wish to include context from an interaction with AI in your comments, it must be disclosed as such.
It must be accompanied by human commentary explaining the relevance and implications of the context.

Questions or proposed changes to this policy can be discussed in a [GitHub issue].

This policy was adapted from [Bitcoin Core's AI policy].

[GitHub issue]: https://github.com/bitcoindevkit/bdk/issues
[Bitcoin Core's AI policy]: https://github.com/bitcoin/bitcoin/blob/master/doc/AI_POLICY.md
