# CI recipes

Recipes that run Source Sentry's two gates in four CI engines. The commands
are those of [cli.md](../docs/cli.md).

| Engine | Recipe | Copy it to |
|---|---|---|
| GitHub Actions | [github-actions.yml](github-actions.yml) | `.github/workflows/source-sentry.yml` |
| GitLab CI | [gitlab-ci.yml](gitlab-ci.yml) | `.gitlab-ci.yml`, or include it from there |
| Forgejo Actions | [forgejo-actions.yml](forgejo-actions.yml) | `.forgejo/workflows/source-sentry.yaml` |
| Woodpecker | the three files in [woodpecker/](woodpecker/) | `.woodpecker/`, under the same names |

> **Status.** None of these recipes has run on real CI yet. Each must be
> tested on its engine before the first release; [Testing before
> release](#testing-before-release) lists what each one needs. Every recipe
> installs a release of Source Sentry pinned by version and checksum, and
> those pins are placeholders until you replace them
> ([Installation](#installation)). Source Sentry has not published a release
> yet.

Every recipe assumes that the repository config declares a `production`
context, as `structure/repository-config.example.yaml` does. Without it,
every production check fails with exit 3, because an unknown context is a
configuration error.

The recipes run on Linux runners only. No Windows variants are made: the
release's acceptance tests cover Source Sentry itself on Windows, while CI
runs on Linux.

## At a glance

| | GitHub Actions | GitLab CI | Forgejo Actions | Woodpecker |
|---|---|---|---|---|
| Report gate | Job `report-gate` | Job `source-sentry-gates`, exit 1 allowed | Job `report-gate` | Workflow `source-sentry-report` |
| Continue gate | Job `continue-gate` | The same job; exit 2 or more fails it | Job `continue-gate` | Workflow `source-sentry-continue` |
| Required | The continue gate's check | The whole pipeline | The continue gate's status | `ci/woodpecker/pr/source-sentry-continue`, on the forge |
| Pull requests checked at | The merge commit | A merged commit on Premium and Ultimate, if enabled; otherwise the source branch's head | The head; Forgejo provides no merge commit | The head |
| Deploy waits for | Production's continue gate | Production's continue gate | Production's continue gate | Production's continue gate, in workflow `source-sentry-deploy` |
| Instant | Captured in `prepare`, passed as a job output | `$CI_PIPELINE_CREATED_AT` | Captured in `prepare`, passed as a job output | `$CI_PIPELINE_CREATED`, converted |
| Full history | `fetch-depth: 0` | `GIT_DEPTH: 0`, `GIT_STRATEGY: clone` | `fetch-depth: 0` | `depth: 0`, `partial: false` |
| Schedule | `on.schedule` in the file | A pipeline schedule, set in the UI | `on.schedule` in the file | A cron job, set in the settings |
| Who is notified | The user who last changed the cron line | The pipeline's creator, presumably the schedule's owner | The repository's owner | Nobody, unless a step is added |
| Binary | Downloaded once, shared as an artifact | Downloaded once, shared as an artifact | Downloaded once, shared as an artifact | Downloaded in each workflow, against the same pins |

## The two gates

The two gates ask two questions of the same result:

- **The report gate**: "Is anything wrong?" It runs `check --fail-on warn`
  and fails on a warning or a failure. Its failing mark prompts a fix, so it
  is **not required**.
- **The continue gate**: "May the build continue?" It runs
  `check --fail-on fail` and fails only on a failure, or on an error. It **is
  required** before merging.

The exit codes, and how each gate treats them:

| Code | Meaning | Report gate (`--fail-on warn`) | Continue gate (`--fail-on fail`) |
|---|---|---|---|
| 0 | Below the threshold | passes | passes |
| 1 | A warning, at the threshold | fails | does not occur |
| 2 | A failure | fails | fails |
| 3 | A configuration or integrity error | fails | fails |
| 4 | The evaluation could not complete | fails | fails |
| 64 | Invalid command-line usage | fails | fails |

A notice always exits 0, so it never fails either gate.

Both gates run against the same commit, config, binary and defaults version,
and at one evaluation instant passed to both through `--at`. The config and
the defaults version come from the commit itself, through the repository
config's `version`. The continue gate never depends on the report gate. On
GitHub, a job that depended on the report gate would be skipped whenever
there is a warning.

### GitHub Actions and Forgejo Actions

Two jobs, `report-gate` and `continue-gate`, each run one invocation. Both
need only `prepare`, which captures the instant and downloads the binary.

The continue gate also carries `if: ${{ always() }}`. GitHub skips a job whose
dependency failed, and branch protection counts a skipped check as passing.
Without `always()`, a failed `prepare` would leave the required check skipped,
and the pull request mergeable. With it, the job runs, its download of the
binary fails, and so does the required check. Forgejo reports a skipped job as
its own `skipped` status. Its documentation does not say whether that status
satisfies a required check, so the Forgejo recipe takes the same precaution.

Because each gate is a separate job with its own checkout, each job first
confirms that it checked out `$GITHUB_SHA`, the commit of the run.

### GitLab CI

GitLab can show a middle state, so one job serves both gates.
`source-sentry-gates` runs
`check --fail-on warn` with `allow_failure: exit_codes: [1]`:

- Exit 0: the job passes.
- Exit 1, a warning: the job shows an orange warning, and the pipeline still
  succeeds. This is the report gate.
- Exit 2 or more: the job fails, so the pipeline fails and the merge is
  blocked. This is the continue gate.

The trade-off, against two jobs that each run one gate:

- **For one job:** one evaluation, so the two answers cannot disagree about
  the commit, the binary or the instant. There is also one job less to run.
- **For one job:** two jobs would look no different. GitLab has no per-job
  required checks, so a separate report job would need `allow_failure` too,
  and would show the same orange warning.
- **Against one job:** the report gate's mark is an orange warning, not a
  red one. The pipeline succeeds, so no failure email is sent, and a
  scheduled pipeline notifies nobody of warnings.
- **Against one job:** the job tolerates exactly exit 1. If the exit codes
  ever changed, this list would have to change with them.

### Woodpecker

Woodpecker reports one status per workflow to the forge. Each gate therefore
has a workflow file of its own, so the forge shows the report
gate's status beside the continue gate's:

| File in `.woodpecker/` | Runs | Its status |
|---|---|---|
| `source-sentry-report.yml` | The report gate | Fails on a warning or worse. Do not require it. |
| `source-sentry-continue.yml` | The continue gate | Fails on a failure or an error. Require it. |
| `source-sentry-deploy.yml` | Production's continue gate, then the deploy, on pushes to the default branch | Shows whether the deploy ran ([Deploying](#deploying)). |

No workflow depends on another, so the continue gate never depends on the
report gate. Every workflow of a pipeline checks out the same commit,
`$CI_COMMIT_SHA`, and confirms it, and reads the same instant,
`$CI_PIPELINE_CREATED`. Woodpecker shares files only between the steps of one
workflow, so each workflow downloads Source Sentry itself, and checks the
archive against the same pinned checksum: one file, verified three times
([Installation](#installation)).

A report gate run as a step with `failure: ignore`, beside the continue
gate, would not do: its failure would appear only in Woodpecker's interface,
because an ignored failure does not change the workflow's status.

## Required checks

### GitHub

Protect the deploying branch with a branch protection rule or a ruleset:

1. Require a pull request before merging.
2. Require status checks to pass, and select **Source Sentry continue
   gate**. Do not select the report gate.
3. Require review from Code Owners (see [Protected policy
   surfaces](#protected-policy-surfaces)).

The check takes the name of the job, `Source Sentry continue gate`. Confirm it
after the first run: GitHub documents that the job's `name` is shown in the
interface, but not, on the pages consulted, that it is the check's name.

### GitLab

1. Under **Settings > Merge requests > Merge checks**, select **Pipelines
   must succeed**.
2. Leave **Skipped pipelines are considered successful** unselected.
3. Protect the default branch.

GitLab has no per-job required checks: every job that is not allowed to fail
is required. Code Owners and "Required approval from code owners" need the
Premium or Ultimate tier.

### Forgejo

In the branch protection rule for the deploying branch:

1. Enable the status check, and add the pattern
   `Source Sentry / Source Sentry continue gate (pull_request)`. Forgejo
   names a job's status `<workflow name> / <job name> (<event>)`, and matches
   patterns as globs.
2. Set the required approvals, and enable **Block merge on official review
   requests**, so that the code owners' review blocks the merge.

### Woodpecker

Woodpecker has no required checks of its own; protection lives on the forge
that Woodpecker reports to. On the forge, require
`ci/woodpecker/pr/source-sentry-continue`, and do not require the report
gate's status.

#### How Woodpecker names the statuses of several workflow files

- **One workflow per file** (documented). A `.woodpecker/` folder makes one
  workflow of each `.yml` or `.yaml` file in it, "each named by the file they
  are defined in". Files in its subfolders are ignored, and the folder takes
  precedence over a `.woodpecker.yml` file.
- **One status per workflow** (documented). "Each workflow will report its
  own status back to your forge."
- **The status's form** (documented in the server's configuration). The
  default is `{{ .context }}/{{ .event }}/{{ .workflow }}`, with the context
  `ci/woodpecker`. A matrix workflow adds `/<axis>`. Administrators can change
  both through `WOODPECKER_STATUS_CONTEXT` and
  `WOODPECKER_STATUS_CONTEXT_FORMAT`.
- **The workflow's name** (read in the source, not documented). It is the
  file's base name without `.yml` or `.yaml`, and without a leading dot.
- **The event** (read in the source, not documented). A pull request is
  written `pr`; other events keep their names, such as `push`.

With the defaults, the three files report:

| File | Pull requests | Pushes to the default branch |
|---|---|---|
| `source-sentry-report.yml` | `ci/woodpecker/pr/source-sentry-report` | `ci/woodpecker/push/source-sentry-report` |
| `source-sentry-continue.yml` | `ci/woodpecker/pr/source-sentry-continue` | `ci/woodpecker/push/source-sentry-continue` |
| `source-sentry-deploy.yml` | none; it does not run | `ci/woodpecker/push/source-sentry-deploy` |

**Unverified:**
- The name and event rules come from Woodpecker's `main` branch, which may
  be ahead of the release a server runs.
- A server's administrator may have changed the format.
- How each forge displays these statuses was not checked.

Confirm the names on the forge after the first run, before requiring one.

## Pull requests: the commit that is checked

Pull requests are checked at their merge commit where the engine provides
one, because the merge commit is what will land.

- **GitHub** provides one. For a pull request, `GITHUB_SHA` is the "last
  merge commit" of `refs/pull/<number>/merge`, and `actions/checkout` checks
  out that commit by default. Its first parent is the base branch. GitHub
  starts no pull request workflow while the pull request has a merge
  conflict, so the required check stays pending and the merge stays blocked.
- **GitLab** provides one on the Premium and Ultimate tiers. Merged results
  pipelines "test a temporary merged commit that combines code from the
  source and target branches". Enable them under **Settings > Merge requests
  > Merge options**; the recipe needs no change. On a conflict, GitLab runs a
  standard merge request pipeline instead. On the Free tier, a merge request
  pipeline runs "on the contents of the source branch only", so it checks the
  source branch's head. Which parent of the temporary merged commit comes
  first was not verified.
- **Forgejo** provides none. A pull request's run uses
  `refs/pull/<number>/head` and the pull request's head commit, and Forgejo
  keeps no merge ref. The recipe therefore checks the head. This was read in
  Forgejo's source; its documentation does not say.
- **Woodpecker** checks the pull request's head. Its integrations with
  GitHub, Forgejo and Gitea record the head as `$CI_COMMIT_SHA`, and the clone
  checks out that commit. Two mechanisms could check a merge result instead,
  and the recipe uses neither:
  - Woodpecker's GitHub integration also sets `$CI_COMMIT_REF` to GitHub's
    `refs/pull/<number>/merge`, by default (`WOODPECKER_GITHUB_MERGE_REF`).
    Checking that ref out would make each of the three workflows fetch it
    separately. GitHub can recompute the merge commit between those fetches,
    so the gates could check different commits.
  - plugin-git's `merge-pull-request` setting merges the target branch
    locally. That makes a commit whose first parent is the head, and whose ID
    differs from one clone to the next.

## The evaluation instant

Every check in a pipeline is measured at one instant, so that the gates
cannot disagree because a deadline fell between them.

- **GitHub and Forgejo** expose no run timestamp in their contexts. The
  `prepare` job captures `date -u +%Y-%m-%dT%H:%M:%SZ` once, writes it to
  `$GITHUB_OUTPUT`, and every gate reads it as
  `needs.prepare.outputs.at`. A gate refuses to run if the value is empty.
- **GitLab** sets `$CI_PIPELINE_CREATED_AT` once per pipeline, in ISO 8601
  in UTC, for example `2022-01-31T16:47:55Z`, which is valid RFC 3339.
- **Woodpecker** sets `$CI_PIPELINE_CREATED` once per pipeline, as UNIX
  seconds, and every workflow of the pipeline receives it. Each check converts
  it with `date -u -d "@$CI_PIPELINE_CREATED" +%Y-%m-%dT%H:%M:%SZ`, which gives
  every gate the same RFC 3339 instant. `$CI_PIPELINE_STARTED` is not used:
  Woodpecker's source sets it in each workflow, when that workflow starts.

A retried gate is measured at the original instant. GitHub's "Re-run failed
jobs" reuses the outputs of the jobs it does not re-run, including
`prepare`; "Re-run all jobs" captures a new instant. A GitLab job retried
within its pipeline should keep the pipeline's creation time, since the
variable belongs to the pipeline, although the documentation does not say
so. This keeps the two gates consistent; a deadline that passed since is
reported by the next run, or by the scheduled one.

## Full history

Source Sentry walks the first-parent history to date each change. When the
walk reaches a shallow boundary, or an object is missing, the evaluation
stops with exit 4, and `check` never fetches. Every recipe therefore fetches
everything:

- **GitHub and Forgejo:** `actions/checkout` with `fetch-depth: 0`.
- **GitLab:** `GIT_DEPTH: '0'`, which fetches the complete history, and
  `GIT_STRATEGY: clone`, which clones afresh in every job. GitLab's
  documentation does not say whether a `fetch` deepens a shallow working copy
  that an earlier job left behind, so the recipe does not rely on it.
- **Woodpecker:** its default clone is both shallow and partial: plugin-git
  enables `partial` by default, which fetches one commit and omits trees that
  the walk reads. Each workflow replaces the clone step with `partial: false`
  and `depth: 0`. A tagged plugin image still receives the forge's credentials
  while the server's list of trusted clone images names the image without a
  tag, as it does by default.

## Scheduled runs

Deadlines and cadence windows pass without anyone committing, so each recipe
runs on a schedule. Scheduled runs use the `default` context only, and run
both gates. Source Sentry sends no notifications itself; CI or the host
does.

- **GitHub:** `on.schedule` runs daily at 06:17 UTC on the latest commit of
  the default branch. A failed scheduled run notifies the user who last
  changed the cron line. In a public repository, GitHub disables scheduled
  workflows after 60 days without activity, and scheduled runs may be delayed
  under load.
- **GitLab:** create a pipeline schedule for the default branch under
  **Build > Pipeline schedules**. It runs with its owner's permissions, and
  pipeline notifications go to the pipeline's creator, which for a schedule
  is presumably its owner. Warnings are allowed failures, so only a failure
  sends an email.
- **Forgejo:** `on.schedule` runs from the default branch only, in UTC unless
  a time zone is set. `enable-email-notifications: true` (Forgejo 12 or
  later) emails the repository's owner, or the organisation's contact
  address, when a scheduled run fails.
- **Woodpecker:** create a cron job in the repository's settings. Without a
  branch, it runs on the default branch. A cron run executes the report and
  continue workflows; the deploy workflow runs on pushes only. Woodpecker
  sends no notifications itself: to be notified, add a step with
  `when: status: [failure]` to each workflow concerned, using the
  notification plugin of your choice.

Any daily time reports a deadline within a day of its passing. Deadlines fall
at the end of a day in the configured time zone, so a run shortly
after that midnight reports them soonest.

How warnings show on a scheduled run differs by engine:

- **GitHub and Forgejo:** the report gate fails on every warning, so a
  scheduled run fails, and notifies, every day while anything warns.
- **Woodpecker:** the report workflow fails the same way, but notifies only
  through a step you add.
- **GitLab:** warnings never notify.

## Deploying

Each recipe adds a production continue gate on pushes to the default branch,
`check --context production --fail-on fail`, at the pipeline's instant. The
deploy placeholder needs that gate only; the default context's
gates do not hold it back.

| Engine | How the deploy waits for production's gate |
|---|---|
| GitHub | `needs: continue-gate-production`; a failed or skipped gate skips the deploy. |
| GitLab | `needs` lists `source-sentry-production-continue` only. GitLab judges a job with `needs` by those jobs alone, so a failing `source-sentry-gates` does not hold the deploy back. |
| Forgejo | As on GitHub. |
| Woodpecker | The workflow `source-sentry-deploy` holds the production gate and the deploy, in that order. Within one workflow, a failed step stops every later step, whatever `depends_on` says. The two therefore cannot share a workflow with the default context's gate. |

The GitLab and Woodpecker behaviours above were read in each engine's source;
their documentation does not state them.

With `structure/repository-config.example.yaml`, a stale privacy policy shows
how this works. In `default`, `stale` only warns, so the pull
request can merge and a staging build can proceed. In `production`, the rule
for the `privacy` class maps `stale` to `fail`, so the production gate exits 2
and the deploy does not run until the policy is reviewed and attested.

Configuration, integrity and operational errors fail production's gate as
well, because no context can downgrade them. Only a failure that
the `default` context reports and `production` does not leaves the deploy
free to run.

To deploy to staging as well, add a gate with `--context staging`, declared
in the repository config, and a deploy that needs it.

## Installation

Every recipe installs a prebuilt release of Source Sentry in one clearly
marked step, which:

1. picks the archive for the runner's architecture, as `uname -m` reports it:
   `source-sentry-<version>-x86_64-unknown-linux-gnu.tar.gz` or
   `source-sentry-<version>-aarch64-unknown-linux-gnu.tar.gz`. Any other
   architecture stops the step;
2. downloads it from
   `https://git.gabmacn.com/gmn/source-sentry/releases/download/v<version>/`,
   with `curl --fail --location` and up to five retries;
3. checks it against the SHA-256 pinned in the recipe, with
   `sha256sum --check --strict`, before extracting anything;
4. extracts it, and runs `source-sentry --version` to show that the binary
   runs on the image.

The archives are those that the product's [README](../README.md#installing)
describes: each holds one folder of the archive's name, with the binary,
`LICENSE` and `THIRD-PARTY-LICENSES.txt`.

- **The pins.** Each install step sets three values:

  | Variable | Placeholder | Replace it with |
  |---|---|---|
  | `SOURCE_SENTRY_VERSION` | `X.Y.Z` | A released version, such as `0.1.0`, without the tag's leading `v` |
  | `SOURCE_SENTRY_SHA256_X86_64` | 64 zeros | The SHA-256 of the x86_64 archive |
  | `SOURCE_SENTRY_SHA256_AARCH64` | 64 zeros | The SHA-256 of the aarch64 archive |

  Until they are replaced, the step stops with a message that names the value
  to replace. Only the runner's own architecture needs a checksum, but
  pinning both lets the recipe move between architectures unchanged. On
  Woodpecker, replace them identically in all three files.
- **Why the checksum is pinned.** Releases are not signed. The
  release's `SHA256SUMS` is served from the same place as the archives, so
  checking an archive against it would detect a damaged download, but not a
  replaced file. A checksum pinned in the recipe ties every run to the bytes
  that were checked when the pin was set: if a published archive ever
  changed, the step would stop rather than run it.
- **Finding the checksums.** Take them from the release's `SHA256SUMS`, and
  cross-check them against the archives downloaded by hand:

  ```sh
  version=0.1.0
  base="https://git.gabmacn.com/gmn/source-sentry/releases/download/v$version"
  curl --fail --location --remote-name "$base/SHA256SUMS"
  for arch in x86_64 aarch64; do
    curl --fail --location --remote-name "$base/source-sentry-$version-$arch-unknown-linux-gnu.tar.gz"
  done
  sha256sum --check --ignore-missing SHA256SUMS   # Both archives must print OK.
  grep -- '-linux-gnu\.tar\.gz$' SHA256SUMS       # The two values to pin.
  ```
- **One binary.** GitHub, GitLab and Forgejo download the archive once per
  pipeline, in `prepare` or `source-sentry-install`, and pass its binary to
  every check as an artifact, so every gate runs the same file.
  That binary suits the architecture of the runner that downloaded it: where
  one runner label covers machines of both architectures, give the jobs a
  label that names one. Woodpecker shares no files between workflows, so
  each of its three workflows downloads and checks the archive itself; with
  the same pins, they run the same bytes on agents of one architecture.
- **Requirements.** The binaries are built on Debian Bookworm, so they need
  glibc 2.36 or newer; musl-based images, such as Alpine, are not supported.
  Installing needs `curl`, `tar`, `sha256sum` and CA certificates, and
  checking needs `git` on the `PATH`. No Rust toolchain or C
  compiler is needed. The images used here provide all of these:
  - GitHub's `ubuntu-24.04` runner, with glibc 2.39;
  - `node:24-trixie` on Forgejo, whose JavaScript actions need Node.js;
  - `buildpack-deps:trixie-scm` on GitLab and Woodpecker, which is Debian
    with `curl`, `git` and CA certificates added.

  Both Debian images are trixie, Debian 13, with glibc 2.41.
- **Access.** The step downloads from `git.gabmacn.com`, and sends no
  credentials. `SOURCE_SENTRY_RELEASES` holds
  the download base: a host that cannot reach `git.gabmacn.com` can point it
  at a copy of a release's files, and the pinned checksums still apply.
- **Building from source instead.** A host that prefers to build can replace
  the install step with Source Sentry's Cargo registry, configured as the
  product's [README](../README.md#installing) describes, or through
  `CARGO_REGISTRIES_GMN_INDEX`:

  ```sh
  cargo install --locked --registry gmn --version <version> source-sentry
  ```

  This compiles Source Sentry and its dependencies, including libgit2's C
  sources, on every run. The image then needs the Rust toolchain
  and a C compiler, and the step takes minutes rather than seconds.

The install step's shell was tested locally on 2026-11-08, in every recipe,
against stand-in archives of the same layout: correct pins on both
architectures, a wrong checksum, the placeholders, an unsupported
architecture, a missing release and a binary that does not run. The download
URL follows Forgejo's route for release files,
`<repository>/releases/download/<tag>/<file>`; it has not been tried against
`git.gabmacn.com`, which has no release yet.

## Protected policy surfaces

Source Sentry's guarantees assume that changes to policy are approved. A
pull request runs its own copy of the workflow on every engine
here:

- **GitHub** runs the workflow file of the pull request's merge commit.
- **GitLab** runs merge request pipelines on the source branch's contents.
- **Forgejo** uses the pull request's own workflow files for trusted authors.
- **Woodpecker** reads the workflow of the commit it runs for.

A pull request could therefore lower its own `--context` or `--fail-on` and
pass its own required check. Only review prevents that, so protect these
files through CODEOWNERS and branch protection:

| Surface | Paths |
|---|---|
| The repository config, presets, remote lockfiles and their configs | `.source-sentry/` |
| Directory configs | every `source-sentry.yaml` |
| Lockfiles, including tombstones | every `source-sentry.lock` |
| Sensitive documents' on-file declarations | the documents themselves, such as `docs/privacy.md` |
| The workflow's context and threshold | the workflow files, and any file they include |
| The protection itself | the CODEOWNERS file |

Class assignments and source declarations live in these files. Owning a
sensitive document with on-file tracking also puts its content under the same
review.

**GitHub and GitLab** use glob patterns. For GitHub, in
`.github/CODEOWNERS`:

```
# Source Sentry policy surfaces. Keep these lines last: the last
# matching pattern takes precedence.
/.source-sentry/                       @example-org/policy-owners
**/source-sentry.yaml                  @example-org/policy-owners
**/source-sentry.lock                  @example-org/policy-owners
/.github/workflows/source-sentry.yml   @example-org/policy-owners
/.github/CODEOWNERS                    @example-org/policy-owners
/docs/privacy.md                       @example-org/legal
```

- **On GitHub**, enable "Require review from Code Owners". The CODEOWNERS
  file must be on the pull request's base branch.
- **On GitLab**, use `.gitlab/CODEOWNERS`, name `/.gitlab-ci.yml` instead of
  the workflow line, and enable "Required approval from code owners" on the
  protected branch. Both need Premium or Ultimate.
- **Stronger options:**
  - GitHub organisation rulesets can require a workflow kept in another
    repository, which a pull request cannot edit. Which plans offer this was
    not verified.
  - GitLab Ultimate's pipeline execution policies run jobs that the project's
    own CI configuration cannot override.

**Forgejo** reads CODEOWNERS from the repository's root, `docs/` or
`.forgejo/`, and its patterns are Go regular expressions, not globs:

```
^\.source-sentry/.*$                        @example-org/policy-owners
(^|.*/)source-sentry\.(yaml|lock)$          @example-org/policy-owners
^\.forgejo/workflows/source-sentry\.yaml$   @example-org/policy-owners
^\.forgejo/CODEOWNERS$                      @example-org/policy-owners
^docs/privacy\.md$                          @example-org/legal
```

The anchors are explicit, because the documentation does not say whether
Forgejo anchors patterns itself. Enable "Block merge on official review
requests", so that a pending code-owner review blocks the merge.

**Woodpecker** relies on the forge's protection for the whole `.woodpecker/`
folder: a new file in it adds a workflow, so own the folder, not only the
three files. Woodpecker's "Require approval for" setting holds pull request
pipelines until someone approves them, by default for forks only. It does not
stop a trusted author's edited workflow; review does.

## Testing before release

The recipes have not run on real CI. The four recipes are part of the
release's acceptance tests, so each must be tested on its engine before the
first release.

### A fixture repository

Every engine needs a repository with the example repository config, whose
`production` context fails a stale `privacy` document, and documents that can
be put in each state below. The production gate and deploy columns describe
the push to the default branch:

| Scenario | Report gate | Continue gate | Production gate | Merge | Deploy |
|---|---|---|---|---|---|
| Every document fresh | passes | passes | passes | allowed | runs |
| An ordinary document stale | fails (1) | passes | passes | allowed | runs |
| The privacy policy stale | fails (1) | passes | fails (2) | allowed | blocked |
| A rule maps a fact to `fail` in `default` only | fails (2) | fails (2) | passes | blocked | runs |
| A malformed lockfile or unknown context | fails (3) | fails (3) | fails (3) | blocked | blocked |
| A shallow clone, forced for the test | fails (4) | fails (4) | fails (4) | blocked | blocked |
| The install step fails | skipped or failed | fails, never skipped | does not run | blocked | blocked |
| A deadline passes with no commit | the scheduled run reports it | | | | |

Each test should also confirm:

- that the install step downloads the pinned archive from
  `git.gabmacn.com`, and stops on a wrong checksum before extracting it;
- that both gates print the same instant and the same commit in their first
  line;
- that the scheduled run starts, and that a failure reaches the expected
  person;
- that the production gate and the deploy run only on pushes to the default
  branch;
- that a pull request is checked at the commit that
  [Pull requests](#pull-requests-the-commit-that-is-checked) describes for
  its engine.

### What each engine still needs

- **GitHub:**
  - the check's name as branch protection lists it;
  - that `always()` keeps the continue gate from being skipped when
    `prepare` fails, and that the merge is then blocked;
  - that a pull request with a merge conflict starts no run, and stays
    blocked;
  - the tar round trip through the artifact;
  - who receives the scheduled run's failure email.
- **GitLab:**
  - that exit 1 shows the orange warning while the pipeline succeeds and the
    merge check passes;
  - that `deploy-production` runs while `source-sentry-gates` fails, as
    GitLab's source implies;
  - that `--at` accepts `$CI_PIPELINE_CREATED_AT`, and that a retried job
    keeps it;
  - the executable bit after the artifact is restored;
  - that merge request pipelines have the full history;
  - that a merged results pipeline, on Premium, sets
    `CI_MERGE_REQUEST_EVENT_TYPE` to `merged_result` and checks the merged
    commit;
  - who receives the schedule's email.
- **Forgejo:**
  - the `docker` runner label and the `node:24-trixie` image;
  - the v3 artifact actions, which declare a Node.js 16 runtime that GitHub
    no longer runs, and `checkout@v4` with `fetch-depth: 0`;
  - the status name, and whether a skipped required status blocks the merge;
  - `always()`;
  - that a pull request's `$GITHUB_SHA` is its head, as the source shows, so
    that the "Confirm the commit" step passes;
  - the scheduled run's email.
- **Woodpecker:**
  - the three statuses' names on the forge, and that the report workflow's
    failure shows there while the continue workflow passes;
  - that every workflow of a pipeline receives the same `$CI_COMMIT_SHA` and
    `$CI_PIPELINE_CREATED`;
  - that the clone is complete: `git rev-parse --is-shallow-repository`
    prints `false`, and no promisor remote is configured;
  - the `$$` escapes and the `date` conversion;
  - that the deploy workflow runs on pushes to the default branch only, and
    deploys while the continue workflow fails;
  - the cron job;
  - cloning a private repository with the pinned plugin image.

## Choices at a glance

- GitLab runs both gates in a single job.
- Woodpecker's report gate has its own workflow file, so the forge shows its
  status.
- Scheduled runs use the default context only.
- Deployment needs production's continue gate only.
- Pull requests are checked at their merge commit, where the engine provides
  one.
- No Windows variants of the recipes are made.

## Interpretations

Where Source Sentry's documented behaviour leaves room, the recipes take the
most conservative reading.

1. **Contexts.** Pull requests, merge requests and
   pushes use `default`; deploying uses `production`.
2. **Production runs its continue gate only.** The default
   context's report gate, in the same pipeline, already shows what is wrong.
3. **One instant for every check in a pipeline**, the production gate
   included.
4. **The same binary** is read as the same file where the engine
   can pass files between jobs. On Woodpecker, which cannot, it is the same
   release archive, checked against the same pinned checksum in each
   workflow.
5. **The working tree, not `--rev`** (see `cli.md`). The recipes
   check the clean checkout, as `check` does by default. Wherever the gates
   run in separate checkouts (GitHub, Forgejo and Woodpecker), each checkout
   is confirmed to be the pipeline's commit.
6. **The continue gate is never skippable** on GitHub and Forgejo, because
   a skipped check can count as passing.
7. **No `--no-cache`.** Fresh checkouts have no cache, and the
   cache holds only immutable facts.
8. **Notifications come from the host.** The Forgejo recipe opts
   into failure emails, which Forgejo leaves off by default.
9. **Full history and full objects.** GitLab also clones afresh
   in every job, and Woodpecker turns off partial cloning.
10. **Woodpecker checks a pull request's head.** The merge
    ref that its GitHub integration exposes is not checked out, because the
    workflows' separate fetches of it could disagree.

## Sources

Each engine's syntax and behaviour was checked on 2026-11-07 against:

- **GitHub Actions:**
  - [contexts](https://docs.github.com/en/actions/reference/workflows-and-actions/contexts)
  - [variables](https://docs.github.com/en/actions/reference/workflows-and-actions/variables)
  - [workflow commands](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands)
  - [workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)
  - [expressions](https://docs.github.com/en/actions/reference/workflows-and-actions/expressions)
  - [events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows)
  - [protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches)
  - [troubleshooting required status checks](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks)
  - [re-running jobs](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs)
  - [code owners](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners)
  - [`pull_request_target`](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target)
  - [ruleset rules](https://docs.github.com/en/enterprise-cloud@latest/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets)
  - the [checkout](https://github.com/actions/checkout), [upload-artifact](https://github.com/actions/upload-artifact) and [download-artifact](https://github.com/actions/download-artifact) actions
  - the [Ubuntu 24.04 runner image](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md)
- **GitLab CI:**
  - [predefined variables](https://docs.gitlab.com/ci/variables/predefined_variables/)
  - [CI/CD YAML syntax](https://docs.gitlab.com/ci/yaml/)
  - [`workflow`](https://docs.gitlab.com/ci/yaml/workflow/)
  - [rules](https://docs.gitlab.com/ci/jobs/job_rules/)
  - [runner Git settings](https://docs.gitlab.com/ci/runners/configure_runners/)
  - [schedules](https://docs.gitlab.com/ci/pipelines/schedules/)
  - [notifications](https://docs.gitlab.com/user/profile/notifications/)
  - [merge checks](https://docs.gitlab.com/user/project/merge_requests/auto_merge/)
  - [merge request pipelines](https://docs.gitlab.com/ci/pipelines/merge_request_pipelines/)
  - [merged results pipelines](https://docs.gitlab.com/ci/pipelines/merged_results_pipelines/)
  - [code owners](https://docs.gitlab.com/user/project/codeowners/)
  - [pipeline execution policies](https://docs.gitlab.com/user/application_security/policies/pipeline_execution_policies/)
  - [environments](https://docs.gitlab.com/ci/environments/)
  - the [pipeline processing source](https://gitlab.com/gitlab-org/gitlab/-/blob/master/app/services/ci/pipeline_processing/atomic_processing_service.rb), for jobs with `needs`
- **Forgejo Actions** (documentation for Forgejo 17.0):
  - [overview](https://forgejo.org/docs/latest/user/actions/overview/)
  - [reference](https://forgejo.org/docs/latest/user/actions/reference/)
  - [actions](https://forgejo.org/docs/latest/user/actions/actions/)
  - [artifacts](https://forgejo.org/docs/latest/user/actions/advanced-features/)
  - [differences from GitHub](https://forgejo.org/docs/latest/user/actions/github-actions/)
  - [pull request security](https://forgejo.org/docs/latest/user/actions/security-pull-request/)
  - [code owners](https://forgejo.org/docs/latest/user/collaboration/pull-requests-and-git-flow/)
  - [runner configuration](https://forgejo.org/docs/latest/admin/actions/configuration/)
  - [the Cargo registry](https://forgejo.org/docs/latest/user/packages/cargo/)
  - the source of [commit statuses](https://codeberg.org/forgejo/forgejo/src/branch/forgejo/services/actions/commit_status.go) and of [pull request runs](https://codeberg.org/forgejo/forgejo/src/branch/forgejo/services/actions/notifier_helper.go)
- **Woodpecker** (documentation for Woodpecker 3.20):
  - [workflow syntax](https://woodpecker-ci.org/docs/usage/workflow-syntax)
  - [workflows](https://woodpecker-ci.org/docs/usage/workflows)
  - [environment](https://woodpecker-ci.org/docs/usage/environment)
  - [cron](https://woodpecker-ci.org/docs/usage/cron)
  - [project settings](https://woodpecker-ci.org/docs/usage/project-settings)
  - [plugin-git](https://github.com/woodpecker-ci/plugin-git/blob/main/docs.md)
  - [server configuration](https://github.com/woodpecker-ci/woodpecker/blob/main/docs/docs/30-administration/10-configuration/10-server.md)
  - the source of the [`when` filter](https://github.com/woodpecker-ci/woodpecker/blob/main/pipeline/frontend/yaml/constraint/constraint.go), [workflow names](https://github.com/woodpecker-ci/woodpecker/blob/main/pipeline/frontend/builder/utils.go), [status names](https://github.com/woodpecker-ci/woodpecker/blob/main/server/forge/common/status.go), [step skipping](https://github.com/woodpecker-ci/woodpecker/blob/main/pipeline/runtime/step.go) and [GitHub pull requests](https://github.com/woodpecker-ci/woodpecker/blob/main/server/forge/github/parse.go)
- **Installation:**
  - [`cargo install`](https://doc.rust-lang.org/cargo/commands/cargo-install.html), for building from the registry instead
  - the [node](https://hub.docker.com/_/node) image, built on [buildpack-deps](https://github.com/docker-library/buildpack-deps), whose `trixie-scm` variant the GitLab and Woodpecker recipes use
