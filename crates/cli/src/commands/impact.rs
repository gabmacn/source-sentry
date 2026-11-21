//! `impact`: the documents that describe given files. It reads declarations
//! and configs only, and it is not a gate.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

use source_sentry_core::claims::Mode;
use source_sentry_core::claims::{
	Diagnostic, DiagnosticKind, Inventory, discover,
};
use source_sentry_core::config::{
	ConfigError, ConfigSet, LoadError, project_of, project_roots,
};
use source_sentry_core::evaluate::settings::EffectiveSettings;
use source_sentry_core::evaluate::unloadable;
use source_sentry_core::impact::{
	Impact, Impacted, Reach, Reached, Searched, impact,
};
use source_sentry_core::model::{DocumentId, RepoPath};
use source_sentry_core::ports::{Revision, Snapshot};
use source_sentry_core::report::ImpactReport;

use crate::cli::{Format, ImpactArgs};
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::Workspace;

/// Runs `impact`.
///
/// # Errors
///
/// Exit 64 for no input, a directory or an unreadable path; 4 when the
/// repository cannot be read. A configuration error that may hide a document
/// does not stop the query: it answers from what it read, lists the errors
/// and exits 3.
pub(super) fn run(
	workspace: &Workspace,
	args: &ImpactArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let from_input = args.paths.iter().filter(|path| is_stdin(path)).count();
	if args.paths.is_empty() && !args.changed && !args.staged {
		return Err(Failure::usage(
			"name the files to look up, `-` to read them from standard input, \
			 or pass `--changed` or `--staged`",
		));
	}
	if from_input > 1 {
		return Err(Failure::usage("`-` may be given only once"));
	}
	if args.nul && from_input == 0 {
		return Err(Failure::usage(
			"`-z` separates the paths read with `-`; pass `-` as well",
		));
	}
	let repository = &workspace.repository;
	if let Some(revision) = &args.rev {
		let snapshot = repository
			.commit_snapshot(revision)
			.map_err(|error| Failure::backend(&error))?;
		answer(workspace, &snapshot, args, printer)
	} else {
		let snapshot = repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		answer(workspace, &snapshot, args, printer)
	}
}

/// Whether a path argument is `-`, which reads paths from standard input.
fn is_stdin(path: &Path) -> bool {
	path.as_os_str() == "-"
}

/// Answers for `snapshot`.
fn answer<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
	args: &ImpactArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let tracked: Vec<RepoPath> =
		snapshot.paths().map_err(|error| Failure::backend(&error))?;
	let roots = project_roots(&tracked);
	let start = project_of(&roots, &workspace.scope);
	let inputs = Inputs::gather(workspace, args, &tracked)?;
	let search = Search::run(workspace, snapshot, &start, &roots)?;
	let answer = impact(&search.searched(), &inputs.all, Reach::Transitive);
	match args.format {
		Format::Json => {
			let report = ImpactReport::new(
				&answer,
				&start,
				&roots,
				&search.unloaded,
				snapshot.revision(),
				&search.hiding,
			);
			let json = report.to_json(&roots).map_err(|error| {
				Failure::new(
					exit::OPERATIONAL,
					format!("cannot write the report: {error}"),
				)
			})?;
			output::data(&json);
		}
		Format::Human if printer.is_quiet() => {
			for line in search.error_lines(workspace) {
				output::write_err(&line);
			}
		}
		Format::Human => printer.print(&render(
			workspace,
			snapshot.revision(),
			&search,
			&answer,
			&inputs,
		)),
	}
	Ok(search.code())
}

/// The files to look up, and where each came from.
struct Inputs {
	/// Every file.
	all: BTreeSet<RepoPath>,
	/// The files named as arguments, which are listed when nothing cites
	/// them; the others are counted.
	named: BTreeSet<RepoPath>,
}

impl Inputs {
	/// Collects the files that `args` names, refusing a directory among the
	/// arguments.
	fn gather(
		workspace: &Workspace,
		args: &ImpactArgs,
		tracked: &[RepoPath],
	) -> Result<Self, Failure> {
		let mut inputs = Self {
			all: BTreeSet::new(),
			named: BTreeSet::new(),
		};
		for argument in &args.paths {
			if is_stdin(argument) {
				inputs.all.extend(read_stdin(args.nul)?);
				continue;
			}
			let path = workspace.repo_path(argument)?;
			if is_directory(workspace, tracked, &path, args.rev.is_none()) {
				return Err(Failure::usage(format!(
					"`{}` is a directory; name the files in it instead",
					argument.display()
				)));
			}
			inputs.named.insert(path.clone());
			inputs.all.insert(path);
		}
		if args.changed || args.staged {
			let changed = workspace
				.repository
				.changed_files(!args.changed)
				.map_err(|error| Failure::backend(&error))?;
			inputs.all.extend(changed);
		}
		Ok(inputs)
	}
}

/// Whether `path` names a directory: the repository root, a directory of
/// tracked files, or, in the working tree, a directory on disk that git
/// does not track as a file itself, as it tracks a submodule.
fn is_directory(
	workspace: &Workspace,
	tracked: &[RepoPath],
	path: &RepoPath,
	working_tree: bool,
) -> bool {
	path.is_root()
		|| tracked
			.iter()
			.any(|file| file != path && file.starts_with(path))
		|| (working_tree
			&& !tracked.contains(path)
			&& workspace.file(path).is_dir())
}

/// Whether `path` names a directory in the commit that `revision` selects,
/// as [`is_directory`] decides: `explain --rev` looks a file up in the
/// commit it explains, whatever the working tree holds there today.
///
/// # Errors
///
/// Exit 4 when the repository cannot be read.
pub(super) fn is_committed_directory(
	workspace: &Workspace,
	revision: &str,
	path: &RepoPath,
) -> Result<bool, Failure> {
	let tracked = workspace
		.repository
		.commit_snapshot(revision)
		.map_err(|error| Failure::backend(&error))?
		.paths()
		.map_err(|error| Failure::backend(&error))?;
	Ok(is_directory(workspace, &tracked, path, false))
}

/// The paths on standard input: one per line, relative to the repository
/// root, as git prints them, or separated by NUL with `-z`. Empty entries
/// are skipped, and a line's carriage return, which some shells add, is
/// dropped.
///
/// # Errors
///
/// Exit 4 when standard input cannot be read, and 64 for an entry that is
/// not a repository path, or that git quoted.
fn read_stdin(nul: bool) -> Result<Vec<RepoPath>, Failure> {
	let mut bytes = Vec::new();
	std::io::stdin().read_to_end(&mut bytes).map_err(|error| {
		Failure::new(
			exit::OPERATIONAL,
			format!("cannot read standard input: {error}"),
		)
	})?;
	parse_paths(&bytes, nul)
}

/// [`read_stdin`]'s parsing.
fn parse_paths(bytes: &[u8], nul: bool) -> Result<Vec<RepoPath>, Failure> {
	let separator = if nul { b'\0' } else { b'\n' };
	let mut paths = Vec::new();
	for entry in bytes.split(|&byte| byte == separator) {
		let entry = if nul {
			entry
		} else {
			entry.strip_suffix(b"\r").unwrap_or(entry)
		};
		if entry.is_empty() {
			continue;
		}
		let text = std::str::from_utf8(entry).map_err(|_| {
			Failure::usage(format!(
				"`{}` on standard input is not UTF-8",
				String::from_utf8_lossy(entry)
			))
		})?;
		if !nul && text.starts_with('"') && text.ends_with('"') && text.len() > 1 {
			return Err(Failure::usage(format!(
				"`{text}` on standard input is quoted, as git quotes unusual \
				 names; pass `-z` to git and to impact"
			)));
		}
		let path = RepoPath::new(text).map_err(|error| {
			Failure::usage(format!("`{text}` on standard input: {error}"))
		})?;
		paths.push(path);
	}
	Ok(paths)
}

/// The searched projects: the one a command runs in and those nested in it,
/// each with its configs and inventory, and the errors that may hide a
/// document from the search.
pub(super) struct Search {
	/// The project the search starts in.
	pub(super) start: RepoPath,
	/// Every project root of the snapshot.
	pub(super) roots: BTreeSet<RepoPath>,
	/// Each searched project whose configs loaded.
	loaded: Vec<(ConfigSet, Inventory)>,
	/// The projects among `start` and those nested in it whose configs could
	/// not be loaded, which were therefore not searched.
	unloaded: BTreeSet<RepoPath>,
	/// The errors that may hide a document: configs that did not load,
	/// lockfiles and declarations that could not be read, and sources that
	/// could not be resolved, without repeats.
	pub(super) hiding: Vec<Diagnostic>,
	/// The configuration error behind each diagnostic of a config that did
	/// not load, so that people see it as `check` shows it.
	config_errors: Vec<(Diagnostic, ConfigError)>,
}

impl Search {
	/// Loads and discovers `start` and every project nested in it, among
	/// `roots`, in `snapshot`. A project whose configs are invalid is left
	/// out and recorded as not searched, and its errors are kept.
	///
	/// # Errors
	///
	/// Exit 4 when the snapshot cannot be read.
	pub(super) fn run<S: Snapshot>(
		workspace: &Workspace,
		snapshot: &S,
		start: &RepoPath,
		roots: &BTreeSet<RepoPath>,
	) -> Result<Self, Failure> {
		let mut search = Self {
			start: start.clone(),
			roots: roots.clone(),
			loaded: Vec::new(),
			unloaded: BTreeSet::new(),
			hiding: Vec::new(),
			config_errors: Vec::new(),
		};
		for project in roots.iter().filter(|root| root.starts_with(start)) {
			let configs = match ConfigSet::load_project(snapshot, project) {
				Ok(configs) => configs,
				Err(LoadError::Invalid(errors)) => {
					search.unloaded.insert(project.clone());
					for error in errors {
						let diagnostics = unloadable(
							snapshot.revision(),
							jiff::Timestamp::UNIX_EPOCH,
							std::slice::from_ref(&error),
						)
						.diagnostics;
						for diagnostic in diagnostics {
							search.hiding.push(diagnostic.clone());
							search.config_errors.push((diagnostic, error.clone()));
						}
					}
					continue;
				}
				Err(error) => return Err(Failure::load(workspace, &error)),
			};
			let inventory = discover(snapshot, &configs)
				.map_err(|error| Failure::backend(&error))?;
			search.hiding.extend(
				inventory
					.diagnostics
					.iter()
					.filter(|diagnostic| hides(diagnostic.kind))
					.cloned(),
			);
			search.loaded.push((configs, inventory));
		}
		// Each project's diagnostics are sorted; one that several projects
		// report, such as an invalid repository config, is kept once.
		let mut kept: Vec<Diagnostic> = Vec::new();
		for diagnostic in std::mem::take(&mut search.hiding) {
			if !kept.contains(&diagnostic) {
				kept.push(diagnostic);
			}
		}
		search.hiding = kept;
		Ok(search)
	}

	/// The searched projects, for [`impact`].
	pub(super) fn searched(&self) -> Vec<Searched<'_>> {
		self
			.loaded
			.iter()
			.map(|(configs, inventory)| Searched { configs, inventory })
			.collect()
	}

	/// The projects outside the start, which were not searched.
	fn outside(&self) -> Vec<&RepoPath> {
		self
			.roots
			.iter()
			.filter(|root| !root.starts_with(&self.start))
			.collect()
	}

	/// Whether `project` was searched: it lies in the start, and its configs
	/// loaded.
	fn has_searched(&self, project: &RepoPath) -> bool {
		project.starts_with(&self.start) && !self.unloaded.contains(project)
	}

	/// The errors that may hide a document, one line each, for people: a
	/// config that did not load as `check` shows its errors, with every path
	/// relative to the effective starting directory, and any other error as
	/// `check` lists diagnostics.
	pub(super) fn error_lines(&self, workspace: &Workspace) -> Vec<String> {
		let mut lines: Vec<String> = Vec::new();
		for diagnostic in &self.hiding {
			let rendered = match self
				.config_errors
				.iter()
				.find(|(known, _)| known == diagnostic)
			{
				Some((_, error)) => {
					let severity = diagnostic.severity();
					vec![format!(
						"{}  {}  {}",
						output::severity(severity.name(), severity.is_fatal()),
						diagnostic.kind.code(),
						exit::config_error(workspace, error)
					)]
				}
				None => super::diagnostic_lines(workspace, [diagnostic]),
			};
			for line in rendered {
				if !lines.contains(&line) {
					lines.push(line);
				}
			}
		}
		lines
	}

	/// The exit code: 3 when an error may hide a document, otherwise 0.
	pub(super) fn code(&self) -> u8 {
		if self.hiding.is_empty() {
			exit::SUCCESS
		} else {
			exit::CONFIGURATION
		}
	}
}

/// Whether a diagnostic of `kind` may hide a document or one of its
/// sources from the search. Every other error leaves the declarations
/// readable: a deleted source, in particular, still matches.
fn hides(kind: DiagnosticKind) -> bool {
	matches!(
		kind,
		DiagnosticKind::InvalidLockfile
			| DiagnosticKind::InvalidDeclaration
			| DiagnosticKind::InvalidSourcePath
			| DiagnosticKind::InvalidPattern
	)
}

/// The human answer: one list of documents, the inputs that nothing cites,
/// the notes on what was not searched, the errors, and the summary.
fn render(
	workspace: &Workspace,
	revision: &Revision,
	search: &Search,
	answer: &Impact,
	inputs: &Inputs,
) -> String {
	let mut text = format!(
		"{} · {}\n",
		describe::count(inputs.all.len(), "file", "files"),
		describe::revision(revision)
	);
	let names: Vec<String> = answer
		.documents
		.iter()
		.map(|document| name(workspace, search, document))
		.collect();
	let width = names
		.iter()
		.map(|name| name.chars().count())
		.max()
		.unwrap_or(0)
		.min(40);
	if !answer.documents.is_empty() {
		text.push('\n');
	}
	for (document, name) in answer.documents.iter().zip(&names) {
		let label = match (&document.reached, document.sealed) {
			(_, true) => "sealed",
			(Reached::Direct(_), false) => "direct",
			(Reached::Via(_), false) => "via   ",
		};
		let _ = writeln!(
			text,
			"{}  {}  {}",
			output::dim(label),
			output::pad(&output::bold(name), name, width),
			output::dim(&summary(document))
		);
		for line in reasons(workspace, document) {
			let _ = writeln!(text, "        {line}");
		}
	}

	let notes = unmatched(workspace, answer, inputs);
	if !notes.is_empty() {
		text.push('\n');
		for note in notes {
			let _ = writeln!(text, "{note}");
		}
	}
	text.push_str(&not_searched(workspace, search, answer));
	if !search.hiding.is_empty() {
		text.push('\n');
		for line in search.error_lines(workspace) {
			let _ = writeln!(text, "{line}");
		}
		let _ = writeln!(
			text,
			"These errors may hide documents that cite these files; the list \
			 above is what could be read."
		);
	}
	text.push('\n');
	text.push_str(&totals(answer, search));
	text
}

/// What nothing cites, whether or not any document was found: each file
/// named as an argument, and a count of the others.
fn unmatched(
	workspace: &Workspace,
	answer: &Impact,
	inputs: &Inputs,
) -> Vec<String> {
	let unmatched: Vec<&RepoPath> = answer
		.inputs
		.iter()
		.filter(|input| !input.matched)
		.map(|input| &input.path)
		.collect();
	let mut notes: Vec<String> = unmatched
		.iter()
		.filter(|path| inputs.named.contains(**path))
		.map(|path| {
			format!("No document cites `{}`.", workspace.display_path(path))
		})
		.collect();
	let counted = unmatched
		.iter()
		.filter(|path| !inputs.named.contains(**path))
		.count();
	if counted > 0 {
		// The count is of the other files when a document or a file named
		// above is listed too.
		let (singular, plural) = if answer.documents.is_empty() && notes.is_empty()
		{
			("file is", "files are")
		} else {
			("other file is", "other files are")
		};
		notes.push(format!(
			"{} cited by no document.",
			describe::count(counted, singular, plural)
		));
	}
	notes
}

/// The notes on what was not searched: the projects outside the start, the
/// projects inside it whose configs could not be loaded, and each file that
/// lies in one of them.
fn not_searched(
	workspace: &Workspace,
	search: &Search,
	answer: &Impact,
) -> String {
	let note = output::severity("note", false);
	let mut lines = Vec::new();
	let outside: Vec<&RepoPath> = search.outside();
	if !outside.is_empty() {
		lines.push(format!(
			"{note}  {} not searched: {}. A document there may still cite these \
			 files; run impact from a project that contains {}.",
			if outside.len() == 1 {
				"Project"
			} else {
				"Projects"
			},
			named(workspace, &outside),
			if outside.len() == 1 { "it" } else { "them" }
		));
	}
	if let Some(unloaded) = unloaded(workspace, search) {
		lines.push(format!("{note}  {unloaded}"));
	}
	for input in &answer.inputs {
		let own = project_of(&search.roots, &input.path);
		if !search.has_searched(&own) {
			lines.push(format!(
				"{note}  `{}` lies in project `{}`, which was not searched.",
				workspace.display_path(&input.path),
				project_name(workspace, &own)
			));
		}
	}
	let mut text = String::new();
	if !lines.is_empty() {
		text.push('\n');
		for line in lines {
			let _ = writeln!(text, "{line}");
		}
	}
	text
}

/// The note on the projects inside the search whose configs could not be
/// loaded, which were not searched, when there are any.
fn unloaded(workspace: &Workspace, search: &Search) -> Option<String> {
	let unloaded: Vec<&RepoPath> = search.unloaded.iter().collect();
	(!unloaded.is_empty()).then(|| {
		format!(
			"{} not searched because {} configs could not be loaded: {}. The \
			 errors are listed below.",
			if unloaded.len() == 1 {
				"Project"
			} else {
				"Projects"
			},
			if unloaded.len() == 1 { "its" } else { "their" },
			named(workspace, &unloaded)
		)
	})
}

/// A document's name: its path from the current directory, or its remote
/// identity, followed by its project when that is not the starting one.
fn name(workspace: &Workspace, search: &Search, document: &Impacted) -> String {
	let shown = workspace.display_document(&document.document);
	if matches!(document.document, DocumentId::Remote(_))
		&& document.project != search.start
	{
		format!(
			"{shown} (project `{}`)",
			project_name(workspace, &document.project)
		)
	} else {
		shown
	}
}

/// A project, for people: its directory from the current directory.
fn project_name(workspace: &Workspace, project: &RepoPath) -> String {
	workspace.display_path(project)
}

/// Every project of `projects`, each quoted, as `check` names the nested
/// projects it did not check.
fn named(workspace: &Workspace, projects: &[&RepoPath]) -> String {
	projects
		.iter()
		.map(|project| format!("`{}`", project_name(workspace, project)))
		.collect::<Vec<_>>()
		.join(", ")
}

/// What a document is, in one line: how it is tracked, then its classes,
/// owners and schedule when it has them.
fn summary(document: &Impacted) -> String {
	let mut parts = vec![match &document.mode {
		Mode::OnFile => "on-file".to_owned(),
		Mode::LocalLockfile => "lockfile".to_owned(),
		Mode::RemoteLockfile { owner } => format!("remote, owner {owner}"),
	}];
	parts.extend(settings_parts(&document.settings));
	parts.join(" · ")
}

/// The settings worth showing: classes and owners when there are any, a
/// grace period when there is one, and a cadence when one is set.
pub(super) fn settings_parts(settings: &EffectiveSettings) -> Vec<String> {
	let mut parts = Vec::new();
	if !settings.classes.value.is_empty() {
		let classes: Vec<&str> = settings
			.classes
			.value
			.iter()
			.map(source_sentry_core::model::ClassName::as_str)
			.collect();
		parts.push(format!("classes {}", classes.join(", ")));
	}
	if !settings.owners.value.is_empty() {
		parts.push(format!("owners {}", settings.owners.value.join(", ")));
	}
	if settings.grace.value > 0 {
		parts.push(format!(
			"grace {}",
			describe::count(settings.grace.value as usize, "day", "days")
		));
	}
	if let Some(cadence) = &settings.cadence.value {
		parts.push(format!(
			"cadence {} from {}",
			cadence.rrule, cadence.dtstart
		));
	}
	parts
}

/// Why a document was reached: each matching file and the source that
/// names it, or the document it was reached through.
fn reasons(workspace: &Workspace, document: &Impacted) -> Vec<String> {
	let mut lines: Vec<String> = match &document.reached {
		Reached::Direct(matches) => matches
			.iter()
			.map(|found| {
				output::dim(&format!(
					"{}, declared as {}",
					workspace.display_path(&found.input),
					found.source
				))
			})
			.collect(),
		Reached::Via(through) => vec![output::dim(&format!(
			"cites {}, {} from a file",
			workspace.display_path(through),
			describe::count(document.depth, "step", "steps")
		))],
	};
	if document.sealed {
		lines.push(output::dim(
			"a sealed record never changes, so these files do not affect it",
		));
	}
	lines
}

/// The last line: `2 direct · 1 through other documents · 1 sealed record ·
/// exit 0`.
fn totals(answer: &Impact, search: &Search) -> String {
	let live = answer.documents.iter().filter(|document| !document.sealed);
	let direct = live.clone().filter(|document| document.depth == 1).count();
	let through = live.filter(|document| document.depth > 1).count();
	let sealed = answer
		.documents
		.iter()
		.filter(|document| document.sealed)
		.count();
	let mut parts = vec![format!("{direct} direct")];
	if through > 0 {
		parts.push(format!("{through} through other documents"));
	}
	if sealed > 0 {
		parts.push(describe::count(sealed, "sealed record", "sealed records"));
	}
	let errors = search.hiding.len();
	if errors > 0 {
		parts.push(describe::count(errors, "error", "errors"));
	}
	parts.push(format!("exit {}", search.code()));
	parts.join(" · ")
}

/// The documents whose sources name `path` directly, searched from
/// `origin`'s project and the projects nested in it, in the snapshot that
/// `rev` selects: the "Cited by" of `explain`.
///
/// # Errors
///
/// Exit 4 when the repository cannot be read.
pub(super) fn citations(
	workspace: &Workspace,
	origin: &RepoPath,
	rev: Option<&str>,
	path: &RepoPath,
) -> Result<(Impact, Search), Failure> {
	let repository = &workspace.repository;
	if let Some(revision) = rev {
		let snapshot = repository
			.commit_snapshot(revision)
			.map_err(|error| Failure::backend(&error))?;
		citations_in(workspace, &snapshot, origin, path)
	} else {
		let snapshot = repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		citations_in(workspace, &snapshot, origin, path)
	}
}

/// [`citations`] in `snapshot`.
fn citations_in<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
	origin: &RepoPath,
	path: &RepoPath,
) -> Result<(Impact, Search), Failure> {
	let tracked = snapshot.paths().map_err(|error| Failure::backend(&error))?;
	let roots = project_roots(&tracked);
	let start = project_of(&roots, origin);
	let search = Search::run(workspace, snapshot, &start, &roots)?;
	let answer = impact(
		&search.searched(),
		&BTreeSet::from([path.clone()]),
		Reach::Direct,
	);
	Ok((answer, search))
}

/// The "Cited by" section of `explain` for `path`: each document that
/// cites it and the sources that name it, the command that follows the
/// citations further, and what was not searched or could not be read.
pub(super) fn cited_by(
	workspace: &Workspace,
	path: &RepoPath,
	answer: &Impact,
	search: &Search,
) -> String {
	let mut text = String::new();
	if answer.documents.is_empty() {
		text.push_str(
			"  no document in the searched projects cites it
",
		);
	}
	let names: Vec<String> = answer
		.documents
		.iter()
		.map(|document| name(workspace, search, document))
		.collect();
	let width = names
		.iter()
		.map(|name| name.chars().count())
		.max()
		.unwrap_or(0)
		.min(40);
	for (document, name) in answer.documents.iter().zip(&names) {
		let mut line = summary(document);
		if document.sealed {
			line.push_str(" · sealed record, which these changes do not affect");
		}
		let _ = writeln!(
			text,
			"  {}  {}",
			output::pad(&output::bold(name), name, width),
			output::dim(&line)
		);
		if let Reached::Direct(matches) = &document.reached {
			for found in matches {
				let _ = writeln!(
					text,
					"      {}",
					output::dim(&format!("declared as {}", found.source))
				);
			}
		}
	}
	if !answer.documents.is_empty() {
		let _ = writeln!(
			text,
			"  `{} impact {}` also lists the documents that cite these in turn",
			workspace.command_prefix(),
			super::shell_word(&workspace.document_argument(path))
		);
	}
	let outside = search.outside();
	if !outside.is_empty() {
		let _ = writeln!(
			text,
			"  {}  {} not searched: {}",
			output::severity("note", false),
			if outside.len() == 1 {
				"Project"
			} else {
				"Projects"
			},
			named(workspace, &outside)
		);
	}
	if let Some(unloaded) = unloaded(workspace, search) {
		let _ = writeln!(text, "  {}  {unloaded}", output::severity("note", false));
	}
	for line in search.error_lines(workspace) {
		let _ = writeln!(text, "  {line}");
	}
	text
}

#[cfg(test)]
mod tests {
	use super::parse_paths;

	fn parsed(bytes: &[u8], nul: bool) -> Vec<String> {
		parse_paths(bytes, nul)
			.unwrap()
			.iter()
			.map(ToString::to_string)
			.collect()
	}

	#[test]
	fn standard_input_takes_lines_or_nul_separated_paths() {
		assert_eq!(
			parsed(b"src/a.rs\r\n\nsrc/b.rs\n", false),
			["src/a.rs", "src/b.rs"]
		);
		assert_eq!(
			parsed(b"src/a b.rs\0src/\"q\".rs\0", true),
			["src/a b.rs", "src/\"q\".rs"]
		);
		assert_eq!(parse_paths(b"", false).unwrap(), []);
		assert_eq!(
			parse_paths(b"\"src/\\303\\251.rs\"\n", false)
				.unwrap_err()
				.code,
			crate::exit::USAGE
		);
		assert_eq!(
			parse_paths(b"../outside.rs\n", false).unwrap_err().code,
			crate::exit::USAGE
		);
	}
}
