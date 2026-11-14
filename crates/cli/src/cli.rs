//! The command line, as `docs/cli.md` specifies it.
//!
//! Parsing only checks the shape of each value. Paths are converted to
//! repository paths once the repository is open, in [`crate::workspace`].

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{ArgMatches, Args, Parser, Subcommand, ValueEnum};
use source_sentry_core::model::{ClassName, ObjectId, RemoteIdentity};
use source_sentry_core::mutate::SourceSpec;
use source_sentry_core::policy::Threshold;

/// Track documentation and records against the sources they describe.
#[derive(Debug, Parser)]
#[command(name = "source-sentry", version, about, max_term_width = 100)]
pub(crate) struct Cli {
	/// Run as if started in PATH, as in git. Relative paths and project
	/// selection start there.
	#[arg(short = 'C', global = true, value_name = "PATH")]
	pub(crate) repo: Option<PathBuf>,
	/// When to use colour. `auto` respects `NO_COLOR` and redirected output.
	#[arg(
		long,
		global = true,
		value_enum,
		default_value_t = ColorWhen::Auto,
		value_name = "WHEN"
	)]
	pub(crate) color: ColorWhen,
	/// Print errors only.
	#[arg(long, global = true)]
	pub(crate) quiet: bool,
	#[command(subcommand)]
	pub(crate) command: Command,
}

impl Cli {
	/// The arguments, with the order of `track`'s `--source` and `--pattern`
	/// taken from `matches`, which derive cannot record.
	pub(crate) fn from_matches(
		matches: &ArgMatches,
	) -> Result<Self, clap::Error> {
		let mut cli = <Self as clap::FromArgMatches>::from_arg_matches(matches)?;
		match (&mut cli.command, matches.subcommand()) {
			(Command::Track(track), Some(("track", sub))) => {
				track.ordered = ordered_sources(sub);
			}
			(Command::Untrack(untrack), Some(("untrack", sub))) => {
				untrack.ordered = ordered_sources(sub);
			}
			_ => {}
		}
		Ok(cli)
	}
}

/// `track`'s sources in the order they were given, which keeps the author's
/// order in frontmatter.
fn ordered_sources(matches: &ArgMatches) -> Vec<SourceSpec> {
	let mut sources: Vec<(usize, SourceSpec)> = Vec::new();
	for (id, pattern) in [("sources", false), ("patterns", true)] {
		let (Some(indices), Some(values)) =
			(matches.indices_of(id), matches.get_many::<String>(id))
		else {
			continue;
		};
		sources.extend(indices.zip(values).map(|(index, value)| {
			let spec = if pattern {
				SourceSpec::Pattern(value.clone())
			} else {
				SourceSpec::Path(value.clone())
			};
			(index, spec)
		}));
	}
	sources.sort_by_key(|(index, _)| *index);
	sources.into_iter().map(|(_, spec)| spec).collect()
}

/// When to colour output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ColorWhen {
	/// Colour a terminal, unless `NO_COLOR` is set.
	Auto,
	/// Always colour.
	Always,
	/// Never colour.
	Never,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
	/// Evaluate every document, unclaimed file and series, and exit with the
	/// gate's code.
	#[command(after_long_help = CHECK_EXIT_CODES)]
	Check(CheckArgs),
	/// Explain one document, file or series in full: its settings and their
	/// origins, its facts, the rules that matched, the timing evidence and its
	/// next deadline.
	Explain(ExplainArgs),
	/// Show each source's changes since its attestation, and the `attest`
	/// command that binds exactly what was shown.
	Diff(DiffArgs),
	/// Record that a document was reviewed against its sources.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Attest(AttestArgs),
	/// Review pending documents on a terminal: on the full screen where it
	/// can be shown, and otherwise one question after another.
	#[command(after_long_help = REVIEW_EXIT_CODES)]
	Review(ReviewArgs),
	/// Seal a lockfile-tracked document as a record, which must never change.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Seal(RecordArgs),
	/// Replace a lockfile entry with a tombstone, keeping a record's seal.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Retire(RecordArgs),
	/// Register a document as unattested, or move documents between
	/// placements with `--from` and `--to`.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	// Boxed, because its options make it much larger than the other commands.
	// A plain comment, since doc comments here become the command's help.
	Track(Box<TrackArgs>),
	/// Remove declared sources from a document's declaration, keeping the
	/// other sources' attestations.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Untrack(UntrackArgs),
	/// Update declarations after moving a document with `git mv`, keeping its
	/// reviews, seal and completion.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Rename(RenameArgs),
	/// Preview adoption: roots, claims and candidates. With paths or `--all`,
	/// register candidates as unattested.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Init(InitArgs),
	/// Migrate lockfiles to the current schema.
	#[command(after_long_help = MUTATION_EXIT_CODES)]
	Migrate(MigrateArgs),
	/// Print the shell completion script for SHELL. It needs no repository.
	#[command(after_long_help = COMPLETIONS_HELP)]
	Completions(CompletionsArgs),
}

/// How to install each shell's completion script.
const COMPLETIONS_HELP: &str = "\
Install the script where your shell looks for completions, for example:
  bash        source-sentry completions bash > ~/.local/share/bash-completion/completions/source-sentry
  zsh         source-sentry completions zsh > ~/.zfunc/_source-sentry
              (with ~/.zfunc in $fpath before compinit runs)
  fish        source-sentry completions fish > ~/.config/fish/completions/source-sentry.fish
  powershell  source-sentry completions powershell | Out-String | Invoke-Expression
              (in your $PROFILE, so that every session loads it)
  elvish      eval (source-sentry completions elvish | slurp)
              (in ~/.config/elvish/rc.elv)

Exit codes:
  0   Printed
  4   Standard output could not be written
  64  Invalid command-line usage";

/// The arguments of `completions`.
#[derive(Debug, Args)]
pub(crate) struct CompletionsArgs {
	/// The shell to complete for.
	#[arg(value_enum, value_name = "SHELL")]
	pub(crate) shell: clap_complete::Shell,
}

/// The exit codes of `check`.
const CHECK_EXIT_CODES: &str = "\
Exit codes:
  0   Nothing reached the threshold; a notice always exits 0
  1   A warning, with --fail-on warn
  2   A failure
  3   A configuration or integrity error
  4   An operational error: the evaluation could not complete
  64  Invalid command-line usage";

/// The exit codes of the commands that change files.
const MUTATION_EXIT_CODES: &str = "\
Exit codes:
  0   Applied, nothing to change, or --dry-run
  1   Not confirmed: declined, or no terminal and no --yes
  2   Refused: the request does not fit the repository
  3   Refused because of a configuration or integrity error
  4   An operational error: the repository could not be read or written
  64  Invalid command-line usage";

/// The exit codes of an interactive review session.
const REVIEW_EXIT_CODES: &str = "\
Exit codes:
  0   Completed or quit; use check as the gate
  1   Standard input or output is not a terminal; use diff and attest
  2   Refused: the request does not fit the repository
  3   Refused because of a configuration or integrity error
  4   An operational error: the repository could not be read or written
  64  Invalid command-line usage";

/// The options that select what is evaluated.
#[derive(Debug, Args)]
pub(crate) struct EvaluationArgs {
	/// The context that decides outcomes.
	#[arg(long, value_name = "NAME", default_value = "default")]
	pub(crate) context: String,
	/// Evaluate a committed snapshot instead of the working tree.
	#[arg(long, value_name = "REVISION")]
	pub(crate) rev: Option<String>,
	/// The evaluation instant, in RFC 3339. By default, now.
	#[arg(long, value_name = "INSTANT", value_parser = parse_instant)]
	pub(crate) at: Option<DateTime<Utc>>,
}

/// `check`.
#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
	#[command(flatten)]
	pub(crate) evaluation: EvaluationArgs,
	/// The outcome that makes the exit code non-zero.
	#[arg(long, value_enum, default_value_t = FailOn::Warn, value_name = "LEVEL")]
	pub(crate) fail_on: FailOn,
	/// Evaluate without the cache. No cache exists yet, so this has no effect
	/// for now.
	#[arg(long)]
	pub(crate) no_cache: bool,
	/// The output format: `json` is shaped as in
	/// `structure/report.example.json`.
	#[arg(long, value_enum, default_value_t = Format::Human, value_name = "FORMAT")]
	pub(crate) format: Format,
}

/// `--fail-on`'s values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum FailOn {
	/// A warning or a failure exits non-zero: the report gate.
	Warn,
	/// Only a failure exits non-zero: the continue gate.
	Fail,
}

impl From<FailOn> for Threshold {
	fn from(value: FailOn) -> Self {
		match value {
			FailOn::Warn => Self::Warn,
			FailOn::Fail => Self::Fail,
		}
	}
}

/// An output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
	/// Text for people.
	Human,
	/// One JSON object.
	Json,
}

/// `explain`.
#[derive(Debug, Args)]
pub(crate) struct ExplainArgs {
	/// A document's path or remote identity, an unclaimed file's path, or a
	/// series' name.
	#[arg(value_name = "SUBJECT")]
	pub(crate) subject: String,
	#[command(flatten)]
	pub(crate) evaluation: EvaluationArgs,
}

/// `diff`.
#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
	/// The document: a relative or absolute filesystem path, or a remote
	/// identity such as `abstract:onboarding-video`.
	#[arg(value_name = "DOCUMENT", value_parser = parse_document)]
	pub(crate) document: DocumentArg,
	/// A source file from the current directory, `/` or `:/`, or a pattern as
	/// declared or `/`-rooted. By default, every source.
	#[arg(long = "source", value_name = "PATH-OR-PATTERN")]
	pub(crate) sources: Vec<String>,
	/// The output format. The JSON `attest` array runs from the repository root.
	#[arg(long, value_enum, default_value_t = Format::Human, value_name = "FORMAT")]
	pub(crate) format: Format,
}

/// The arguments of `review`.
#[derive(Debug, Args)]
pub(crate) struct ReviewArgs {
	/// Limit the session to these documents, relative to the current directory,
	/// or remote identities. By default, every document needing review.
	#[arg(value_name = "DOCUMENT", value_parser = parse_document)]
	pub(crate) documents: Vec<DocumentArg>,
	/// The context that decides the order of outcomes.
	#[arg(long, value_name = "NAME", default_value = "default")]
	pub(crate) context: String,
	/// Who reviewed. By default, `source-sentry.reviewer` in git's
	/// configuration, or else git's `user.name`.
	#[arg(long, value_name = "NAME")]
	pub(crate) reviewer: Option<String>,
	/// Ask one question after another instead of taking over the screen.
	#[arg(long)]
	pub(crate) plain: bool,
	/// Also ask about documents that are stale but not due yet, in the line
	/// mode. The full screen always lists them, marked as not due.
	#[arg(long)]
	pub(crate) all: bool,
}

/// The arguments of `attest`.
#[derive(Debug, Args)]
pub(crate) struct AttestArgs {
	/// The document: a relative or absolute filesystem path, or a remote
	/// identity.
	#[arg(value_name = "DOCUMENT", value_parser = parse_document)]
	pub(crate) document: DocumentArg,
	/// A source file from the current directory, `/` or `:/`, or a pattern as
	/// declared or `/`-rooted. By default, every source.
	#[arg(long = "source", value_name = "PATH-OR-PATTERN")]
	pub(crate) sources: Vec<String>,
	/// What the reviewer approved: a path from the current directory, `/` or `:/`,
	/// as `path@oid`, or
	/// `path@absent` for a removal. When given, the snapshot must match them
	/// exactly; otherwise the current state is recorded.
	#[arg(long = "expect", value_name = "PATH@STATE", value_parser = parse_expectation)]
	pub(crate) expectations: Vec<ExpectationArg>,
	/// Who reviewed. By default, `source-sentry.reviewer` in git's
	/// configuration, or else git's `user.name`.
	#[arg(long, value_name = "NAME")]
	pub(crate) reviewer: Option<String>,
	/// Also record that the whole document was reread.
	#[arg(long)]
	pub(crate) reread: bool,
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
}

/// `seal` and `retire`.
#[derive(Debug, Args)]
pub(crate) struct RecordArgs {
	/// The document: a relative or absolute filesystem path, or a remote
	/// identity.
	#[arg(value_name = "DOCUMENT", value_parser = parse_document)]
	pub(crate) document: DocumentArg,
	/// Who seals or retires it. By default, `source-sentry.reviewer` in
	/// git's configuration, or else git's `user.name`.
	#[arg(long, value_name = "NAME")]
	pub(crate) by: Option<String>,
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
}

/// `rename`: both arguments are filesystem paths, with remote identities
/// parsed so that the command can explain their refusal.
#[derive(Debug, Args)]
pub(crate) struct RenameArgs {
	/// The former document path. Move the file first with `git mv`.
	#[arg(value_name = "OLD", value_parser = parse_document)]
	pub(crate) old: DocumentArg,
	/// The new path, which must exist and be tracked by git.
	#[arg(value_name = "NEW", value_parser = parse_document)]
	pub(crate) new: DocumentArg,
	/// Who renames it. By default, `source-sentry.reviewer` in git's
	/// configuration, or else git's `user.name`.
	#[arg(long, value_name = "NAME")]
	pub(crate) by: Option<String>,
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
}

/// The arguments of `track`.
#[derive(Debug, Args)]
pub(crate) struct TrackArgs {
	/// The document to register; with `--from`, every document to move.
	#[arg(value_name = "DOCUMENT", required = true, value_parser = parse_document)]
	pub(crate) documents: Vec<DocumentArg>,
	/// Register in the document's own frontmatter.
	#[arg(long, conflicts_with_all = ["lockfile", "remote"])]
	pub(crate) on_file: bool,
	/// Register in a local lockfile: the nearest one by default, or the one
	/// in DIRECTORY, a relative or absolute filesystem path.
	#[expect(
		clippy::option_option,
		reason = "clap's shape for a flag whose value is optional: absent, 		          given alone, or given with a directory"
	)]
	#[arg(
		long,
		num_args = 0..=1,
		value_name = "DIRECTORY",
		conflicts_with = "remote"
	)]
	pub(crate) lockfile: Option<Option<PathBuf>>,
	/// Register a remote document in the lockfile of OWNER.
	#[arg(long, value_name = "OWNER")]
	pub(crate) remote: Option<String>,
	/// A link to a remote document, which never changes its identity.
	#[arg(long, value_name = "URL")]
	pub(crate) url: Option<String>,
	/// A source file, relative to the current directory, or rooted at `/`
	/// (the document project) or `:/` (the repository).
	#[arg(long = "source", value_name = "PATH")]
	pub(crate) sources: Vec<String>,
	/// A glob from the current directory, `/` or `:/`, whose every match is a source.
	#[arg(long = "pattern", value_name = "GLOB")]
	pub(crate) patterns: Vec<String>,
	/// A class, which the project config must declare.
	#[arg(long = "class", value_name = "NAME", value_parser = parse_class)]
	pub(crate) classes: Vec<ClassName>,
	/// An owner identifier.
	#[arg(long = "owner", value_name = "ID")]
	pub(crate) owners: Vec<String>,
	/// Whole days the document may lag behind a changed source.
	#[arg(long, value_name = "DAYS")]
	pub(crate) grace: Option<u32>,
	/// A cadence: its first due date, and an RFC 5545 recurrence rule.
	#[arg(
		long,
		num_args = 2,
		value_names = ["DTSTART", "RRULE"],
		conflicts_with = "no_cadence"
	)]
	pub(crate) cadence: Option<Vec<String>>,
	/// Remove an inherited cadence: writes `cadence: null`.
	#[arg(long)]
	pub(crate) no_cadence: bool,
	/// Move the documents from this placement: `on-file`,
	/// `lockfile:<directory>` or `remote:<owner>` or `remote:<owner>@<project-directory>`.
	#[arg(
		long,
		value_name = "PLACEMENT",
		value_parser = parse_placement,
		requires = "to",
		conflicts_with_all = [
			"on_file", "lockfile", "remote", "url", "sources", "patterns",
			"classes", "owners", "grace", "cadence", "no_cadence",
		]
	)]
	pub(crate) from: Option<PlacementArg>,
	/// Move the documents to this placement.
	#[arg(
		long,
		value_name = "PLACEMENT",
		value_parser = parse_placement,
		requires = "from"
	)]
	pub(crate) to: Option<PlacementArg>,
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
	/// `--source` and `--pattern` together, in the order given.
	#[arg(skip)]
	pub(crate) ordered: Vec<SourceSpec>,
}

/// The arguments of `untrack`.
#[derive(Debug, Args)]
pub(crate) struct UntrackArgs {
	/// The document whose declaration loses sources.
	#[arg(value_name = "DOCUMENT", value_parser = parse_document)]
	pub(crate) document: DocumentArg,
	/// A declared source to remove, written as `track` takes it.
	#[arg(long = "source", value_name = "PATH")]
	pub(crate) sources: Vec<String>,
	/// A declared pattern to remove, as declared or `/`-rooted.
	#[arg(long = "pattern", value_name = "GLOB")]
	pub(crate) patterns: Vec<String>,
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
	/// `--source` and `--pattern` together, in the order given.
	#[arg(skip)]
	pub(crate) ordered: Vec<SourceSpec>,
}

/// The arguments of `init`.
#[derive(Debug, Args)]
pub(crate) struct InitArgs {
	/// Candidates to register, as relative or absolute filesystem paths.
	/// Without paths or `--all`, `init` only previews.
	#[arg(value_name = "PATH", conflicts_with = "all")]
	pub(crate) paths: Vec<PathBuf>,
	/// Register every candidate.
	#[arg(long)]
	pub(crate) all: bool,
	/// A class to assign to every registered document.
	#[arg(long = "class", value_name = "NAME", value_parser = parse_class)]
	pub(crate) classes: Vec<ClassName>,
	/// An owner to assign to every registered document.
	#[arg(long = "owner", value_name = "ID")]
	pub(crate) owners: Vec<String>,
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
}

/// `migrate`.
#[derive(Debug, Args)]
pub(crate) struct MigrateArgs {
	#[command(flatten)]
	pub(crate) confirm: ConfirmArgs,
}

/// How a command that changes files is confirmed.
#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct ConfirmArgs {
	/// Print the plan only.
	#[arg(long)]
	pub(crate) dry_run: bool,
	/// Apply without asking.
	#[arg(long)]
	pub(crate) yes: bool,
}

/// A document as named on the command line: a remote identity, or a relative
/// or absolute filesystem path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DocumentArg {
	/// A remote identity, such as `abstract:onboarding-video`.
	Remote(RemoteIdentity),
	/// A filesystem path, relative to the current directory or absolute.
	Path(PathBuf),
}

/// The prefixes that make an argument a remote identity.
const IDENTITY_KINDS: &[&str] = &["abstract", "google-drive", "canva", "miro"];

/// Reads a document argument. Only the four identity prefixes make a remote
/// identity, so a Windows path such as `C:\docs\a.md`, and a file name
/// containing `:`, stay paths; `./abstract:x` names a file.
pub(crate) fn parse_document(value: &str) -> Result<DocumentArg, String> {
	match value.split_once(':') {
		Some((kind, _)) if IDENTITY_KINDS.contains(&kind) => value
			.parse::<RemoteIdentity>()
			.map(DocumentArg::Remote)
			.map_err(|error| error.to_string()),
		_ if value.is_empty() => Err("the document is empty".to_owned()),
		_ => Ok(DocumentArg::Path(PathBuf::from(value))),
	}
}

/// A placement as written after `--from` and `--to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlacementArg {
	/// `on-file`.
	OnFile,
	/// `lockfile:<directory>`, the directory relative to the current one.
	Lockfile(PathBuf),
	/// `remote:<owner>` or `remote:<owner>@<project-directory>`.
	Remote {
		/// The owner identifier, which cannot contain `@`.
		owner: String,
		/// An explicit project root relative to the effective starting directory.
		project: Option<PathBuf>,
	},
}

fn parse_placement(value: &str) -> Result<PlacementArg, String> {
	let invalid = || {
		format!(
			"`{value}` is not a placement; write `on-file`, \
			 `lockfile:<directory>` or `remote:<owner>` or `remote:<owner>@<project-directory>`"
		)
	};
	if value == "on-file" {
		return Ok(PlacementArg::OnFile);
	}
	match value.split_once(':') {
		Some(("lockfile", directory)) if !directory.is_empty() => {
			Ok(PlacementArg::Lockfile(PathBuf::from(directory)))
		}
		Some(("remote", owner)) if !owner.is_empty() => {
			let (owner, project) = owner
				.split_once('@')
				.map_or((owner, None), |(owner, project)| {
					(owner, Some(PathBuf::from(project)))
				});
			if owner.is_empty()
				|| project
					.as_ref()
					.is_some_and(|path| path.as_os_str().is_empty())
			{
				return Err(invalid());
			}
			Ok(PlacementArg::Remote {
				owner: owner.to_owned(),
				project,
			})
		}
		_ => Err(invalid()),
	}
}

/// Reads an instant in RFC 3339, such as `2026-11-07T18:00:00Z`.
fn parse_instant(value: &str) -> Result<DateTime<Utc>, String> {
	DateTime::parse_from_rfc3339(value)
		.map(|instant| instant.with_timezone(&Utc))
		.map_err(|error| {
			format!("`{value}` is not an RFC 3339 instant, such as 2026-11-07T18:00:00Z ({error})")
		})
}

/// An expectation whose path still has its command-line spelling.
#[derive(Clone, Debug)]
pub(crate) struct ExpectationArg {
	/// The path from the current directory, `/` or `:/`.
	pub(crate) path: PathBuf,
	/// The expected oid, or absence.
	pub(crate) oid: Option<ObjectId>,
}

/// Reads the expected state while retaining the path for workspace resolution.
fn parse_expectation(value: &str) -> Result<ExpectationArg, String> {
	let (path, state) = value.rsplit_once('@').ok_or_else(|| {
		format!("`{value}` has no state; write `<path>@<oid>`, or `<path>@absent` for a removal")
	})?;
	if path.is_empty() {
		return Err(format!("`{value}`: the path is empty"));
	}
	let oid = if state == "absent" {
		None
	} else {
		Some(state.parse().map_err(|error| {
			format!("`{value}`: {error}; or write `absent` for a removal")
		})?)
	};
	Ok(ExpectationArg {
		path: PathBuf::from(path),
		oid,
	})
}

fn parse_class(value: &str) -> Result<ClassName, String> {
	ClassName::new(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_known_prefixes_make_identities() {
		assert!(matches!(
			parse_document("abstract:onboarding-video"),
			Ok(DocumentArg::Remote(_))
		));
		assert!(matches!(
			parse_document("google-drive:1a2B3c"),
			Ok(DocumentArg::Remote(_))
		));
		for path in [
			r"C:\docs\a.md",
			"C:/docs/a.md",
			"notes:2026.md",
			"./abstract:x",
		] {
			assert_eq!(
				parse_document(path),
				Ok(DocumentArg::Path(PathBuf::from(path))),
				"{path}"
			);
		}
		assert!(parse_document("abstract:").is_err());
		assert!(parse_document("miro:a b").is_err());
	}

	#[test]
	fn placements_are_parsed_from_their_written_forms() {
		assert_eq!(parse_placement("on-file"), Ok(PlacementArg::OnFile));
		assert_eq!(
			parse_placement("lockfile:docs"),
			Ok(PlacementArg::Lockfile(PathBuf::from("docs")))
		);
		assert_eq!(
			parse_placement("remote:legal"),
			Ok(PlacementArg::Remote {
				owner: "legal".to_owned(),
				project: None
			})
		);
		for invalid in ["lockfile", "lockfile:", "remote:", "elsewhere"] {
			assert!(parse_placement(invalid).is_err(), "{invalid}");
		}
	}

	#[test]
	fn the_command_line_is_consistent() {
		use clap::CommandFactory as _;
		Cli::command().debug_assert();
	}
}
