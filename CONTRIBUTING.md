# Contributing to Source Sentry

Thank you for considering a contribution. Issues and pull requests are both
welcome.

## Where things happen

- **Issues** are tracked at
  <https://git.gabmacn.com/gmn/source-sentry/issues>. Please search before
  opening a new one. To open one, sign in there with your GitHub account:
  the site does not create accounts of its own, so its sign-up page says
  that registration is disabled.
- **Pull requests** are welcome on the same repository, and on the GitHub
  mirror, <https://github.com/gabmacn/source-sentry>. Issues opened on GitHub
  may go unnoticed.

## How a pull request lands

Source Sentry is developed in a larger private repository, and both public
repositories are mirrors of it. A pull request therefore lands in three
steps:

1. **Review here.** The discussion happens on your pull request, as usual.
   The public repositories run no CI, so run the checks below yourself.
2. **Import.** Once your pull request is accepted, the maintainer imports it
   into the private repository, where it runs through the full test suite.
   Small follow-up changes may be added at this point.
3. **Export.** The change is published back to this repository as a merge of
   your own commits. Your pull request then shows as **merged**, and your
   commits, with you as their author, become part of the history.

So please do not be surprised if a few minutes pass between approval and
the merge. Nothing is lost, and the credit stays yours.

If you push new commits after your pull request was imported, they are not
part of that import. Mention it in the pull request, and the maintainer will
import them as well.

## Before you open a pull request

- **Build and test:** run the commands in the README's "Building and testing"
  section. Every one of them must pass.
- **Keep changes focused:** one topic per pull request makes review faster.
- **Discuss changes in behaviour first.** For anything beyond a small fix,
  open an issue before writing code. A change to how Source Sentry behaves is
  welcome, but it should be agreed deliberately before it is written.

## Code style

- **Formatting** is `rustfmt`, with the settings in `rustfmt.toml`: hard tabs,
  80 columns.
- **Lints.** Clippy runs with warnings denied, using the lints in
  `Cargo.toml`. Production code never uses `unwrap`. An `expect` must state why
  it cannot fail. A lint exception uses `#[expect(lint, reason = "…")]` on the
  narrowest item.
- **Documentation.** Public items are documented, with `# Errors` and
  `# Panics` sections where they apply.
- **Tests.** Each change comes with tests that would fail without it. The core
  has in-memory fakes of the repository in `crates/core/src/testing.rs`; the git
  backend and the CLI test against real temporary repositories.
- **Dependencies.** A new dependency needs a comment in `Cargo.toml` naming the
  job it serves.

## Licence

By contributing, you agree that your contributions are licensed under the MIT
licence, as the rest of the project is.
