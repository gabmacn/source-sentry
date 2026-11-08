//! Resolution: effective settings with their origins, roots, and path
//! selection.

use std::collections::BTreeMap;
use std::fmt;

use super::error::Position;
use super::load::{ConfigSet, Node};
use super::parse::{AdapterMapping, Parsed};
use super::pattern::{PathPattern, SeriesPattern};
use super::{BUILT_IN_DOCUMENT_TYPES, DEFAULT_CONTEXT};
use crate::adapters::Adapter;
use crate::model::{
	Cadence, ClassName, Context, Lifecycle, RepoPath, Rule, Schedule, VersionKind,
};
use crate::ports::Revision;

/// The repository root, as a directory that outlives every borrow.
static ROOT: RepoPath = RepoPath::ROOT;

/// The scope that consumes a setting.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
	/// The built-in defaults, which apply everywhere.
	BuiltIn,
	/// The repository config, which applies to the whole repository.
	Repository,
	/// The directory config of this directory, which applies to the
	/// directory and everything below it.
	Directory(RepoPath),
	/// The paired config of this remote owner, which applies to the
	/// documents in its lockfile.
	Remote(String),
}

impl fmt::Display for Scope {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::BuiltIn => formatter.write_str("the built-in defaults"),
			Self::Repository => formatter.write_str("the repository"),
			Self::Directory(directory) => write!(formatter, "directory {directory}"),
			Self::Remote(owner) => write!(formatter, "remote owner {owner}"),
		}
	}
}

/// Where a setting's value comes from.
///
/// The snapshot it was read from is [`ConfigSet::revision`], which is the
/// same for every origin of one set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
	/// The file that writes the value, or `None` for the built-in defaults.
	pub file: Option<RepoPath>,
	/// Where in `file` the value is written.
	pub position: Option<Position>,
	/// When `file` is a preset: the config that imports it, then every preset
	/// on the way to `file`, outermost first. Empty otherwise.
	pub import_chain: Vec<RepoPath>,
	/// The scope that consumes the value: the config that writes it, or that
	/// imports the preset that writes it.
	pub scope: Scope,
}

impl Origin {
	/// The origin of a built-in default.
	#[must_use]
	pub fn built_in() -> Self {
		Self {
			file: None,
			position: None,
			import_chain: Vec::new(),
			scope: Scope::BuiltIn,
		}
	}
}

impl fmt::Display for Origin {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		let Some(file) = &self.file else {
			return formatter.write_str("the built-in defaults");
		};
		write!(formatter, "{file}")?;
		if let Some(position) = self.position {
			write!(formatter, ":{position}")?;
		}
		if !self.import_chain.is_empty() {
			let chain = self
				.import_chain
				.iter()
				.map(ToString::to_string)
				.collect::<Vec<_>>()
				.join(" → ");
			write!(formatter, ", imported through {chain}")?;
		}
		write!(formatter, ", for {}", self.scope)
	}
}

/// A value and its origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved<T> {
	/// The value.
	pub value: T,
	/// Where it comes from.
	pub origin: Origin,
}

/// A class that the repository config declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassDefinition {
	/// The class's description, if any.
	pub description: Option<String>,
	/// Where it is declared.
	pub origin: Origin,
}

/// A context that the repository config declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextDefinition {
	/// The context, as declared.
	pub context: Context,
	/// Where it is declared.
	pub origin: Origin,
}

/// Defaults for the documents of a scope: assigned classes, owners and the
/// schedule. `None` means that nothing sets the value, since the built-in
/// defaults set none of them.
///
/// `grace` and `cadence` resolve separately, as two settings, so a config
/// that sets only `schedule.grace` keeps the inherited cadence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentDefaults {
	/// Classes assigned to documents, including unclaimed files.
	pub classes: Option<Resolved<Vec<ClassName>>>,
	/// Owner identifiers.
	pub owners: Option<Resolved<Vec<String>>>,
	/// Whole days a document may lag behind a changed source.
	pub grace: Option<Resolved<u32>>,
	/// The cadence that due dates fall on. A value of `None` means that a config
	/// removed an inherited cadence with `cadence: null`. Its rule is kept as
	/// written; the schedule module validates it.
	pub cadence: Option<Resolved<Option<Cadence>>>,
}

impl DocumentDefaults {
	/// Applies one contribution's values, which override earlier ones.
	fn apply(&mut self, contribution: &Contribution<'_>) {
		let file = contribution.file;
		if let Some(classes) = &file.classes {
			self.classes = Some(Resolved {
				value: classes
					.value
					.iter()
					.map(|class| class.value.clone())
					.collect(),
				origin: contribution.origin(classes.position),
			});
		}
		if let Some(owners) = &file.owners {
			self.owners = Some(Resolved {
				value: owners.value.clone(),
				origin: contribution.origin(owners.position),
			});
		}
		if let Some(schedule) = &file.schedule {
			if let Some(grace) = schedule.value.grace {
				self.grace = Some(Resolved {
					value: grace,
					origin: contribution.origin(schedule.position),
				});
			}
			// An absent cadence inherits; `null` removes.
			if !schedule.value.cadence.is_inherit() {
				self.cadence = Some(Resolved {
					value: schedule.value.cadence.as_set().cloned(),
					origin: contribution.origin(schedule.position),
				});
			}
		}
	}
}

/// A series in effect for a directory.
///
/// Its identity is its scope plus its name: a series that a directory
/// inherits is the same series, never a new one. Within one scope, a later
/// definition of the same name replaces an earlier one, in resolution order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Series {
	/// The series' name.
	pub name: String,
	/// The scope of the config that defines or imports it.
	pub scope: Scope,
	/// The directory its pattern is relative to: that config's directory, or
	/// the repository root for the repository config.
	pub anchor: RepoPath,
	/// The pattern, with its `version` group.
	pub pattern: SeriesPattern,
	/// How versions are parsed and ordered.
	pub version_kind: VersionKind,
	/// What happens to older entries.
	pub lifecycle: Lifecycle,
	/// The series' own schedule, as written, if any.
	pub schedule: Option<Schedule>,
	/// Where the series is defined.
	pub origin: Origin,
}

impl Series {
	/// The version `path` carries in this series, when the pattern matches
	/// the whole of `path` relative to [`Series::anchor`]. Whether `path`'s
	/// directory is within the series' reach is
	/// [`DirectorySettings::series`]'s concern.
	#[must_use]
	pub fn version_of<'p>(&self, path: &'p RepoPath) -> Option<&'p str> {
		let relative = path.strip_prefix(&self.anchor)?;
		if relative.is_empty() {
			return None;
		}
		self.pattern.version(relative)
	}
}

/// The effective settings of one directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectorySettings {
	/// The directory these settings apply to.
	pub directory: RepoPath,
	/// Classes, owners and the schedule.
	pub defaults: DocumentDefaults,
	/// The patterns of files that are eligible as documents. The last config
	/// that sets `documents.include` replaces the list as a whole.
	pub include: Resolved<Vec<PathPattern>>,
	/// Every ignore pattern that reaches the directory, in resolution order. The
	/// last one that matches a path decides.
	pub ignore: Vec<Resolved<PathPattern>>,
	/// Every adapter mapping that reaches the directory, in resolution order,
	/// starting with the built-in ones. The last one that matches a path decides.
	pub adapters: Vec<Resolved<AdapterMapping>>,
	/// Rules added to contexts by the configs that reach the directory, by
	/// context name, each with its origin. The policy module combines them with
	/// the context's own rules by maximum severity.
	pub rules: BTreeMap<String, Vec<Resolved<Rule>>>,
	/// The series in effect, sorted by scope and name.
	pub series: Vec<Series>,
}

/// Whether a file is eligible as a document, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility<'a> {
	/// No `include` pattern matches the file.
	NotIncluded,
	/// An `include` pattern matches, and no ignore pattern excludes the file.
	Eligible {
		/// The first `include` pattern that matches.
		include: &'a PathPattern,
		/// The exception that brought the file back, when the last matching
		/// ignore pattern starts with `!`.
		exception: Option<&'a Resolved<PathPattern>>,
	},
	/// The last matching ignore pattern excludes the file.
	Ignored {
		/// That pattern.
		by: &'a Resolved<PathPattern>,
	},
}

impl Eligibility<'_> {
	/// Whether the file is eligible.
	#[must_use]
	pub fn is_eligible(&self) -> bool {
		matches!(self, Self::Eligible { .. })
	}
}

impl DirectorySettings {
	/// Whether `path`, a file in [`DirectorySettings::directory`], is eligible as
	/// a document: matched by `include` and not excluded by `ignore`. A pattern
	/// matches a file when it matches the file or a directory that contains it;
	/// among the ignore patterns, the last one that matches decides.
	///
	/// For a file in a deeper directory, use that directory's settings, which
	/// include the deeper configs.
	#[must_use]
	pub fn eligibility(&self, path: &RepoPath) -> Eligibility<'_> {
		let Some(include) = self
			.include
			.value
			.iter()
			.find(|pattern| pattern.matches(path))
		else {
			return Eligibility::NotIncluded;
		};
		match self
			.ignore
			.iter()
			.rev()
			.find(|ignore| ignore.value.matches(path))
		{
			Some(ignore) if !ignore.value.is_negated() => {
				Eligibility::Ignored { by: ignore }
			}
			exception => Eligibility::Eligible { include, exception },
		}
	}

	/// The adapter mapping for `path`, a file in
	/// [`DirectorySettings::directory`]: the last one that matches it, or `None`
	/// when none does. Eligibility is a separate question.
	#[must_use]
	pub fn adapter(&self, path: &RepoPath) -> Option<&Resolved<AdapterMapping>> {
		self
			.adapters
			.iter()
			.rev()
			.find(|mapping| mapping.value.pattern.matches(path))
	}

	/// Every series in effect whose pattern matches `path`, with the version it
	/// carries. More than one is a configuration error for the series module to
	/// report.
	#[must_use]
	pub fn series_of<'p>(&self, path: &'p RepoPath) -> Vec<(&Series, &'p str)> {
		self
			.series
			.iter()
			.filter_map(|series| {
				series.version_of(path).map(|version| (series, version))
			})
			.collect()
	}

	/// The built-in defaults, before any config.
	fn built_in(directory: &RepoPath, built_in: &BuiltIn) -> Self {
		let origin = Origin::built_in();
		Self {
			directory: directory.clone(),
			defaults: DocumentDefaults::default(),
			include: Resolved {
				value: built_in.include.clone(),
				origin: origin.clone(),
			},
			ignore: Vec::new(),
			adapters: built_in
				.adapters
				.iter()
				.map(|mapping| Resolved {
					value: mapping.clone(),
					origin: origin.clone(),
				})
				.collect(),
			rules: BTreeMap::new(),
			series: Vec::new(),
		}
	}

	/// Applies one contribution, in resolution order.
	fn apply(&mut self, contribution: &Contribution<'_>) {
		self.defaults.apply(contribution);
		let file = contribution.file;
		if let Some(documents) = &file.documents {
			let documents = &documents.value;
			if let Some(include) = &documents.include {
				self.include = Resolved {
					value: include
						.value
						.iter()
						.map(|pattern| pattern.value.anchored_at(contribution.anchor))
						.collect(),
					origin: contribution.origin(include.position),
				};
			}
			for pattern in documents.ignore.iter().flat_map(|ignore| &ignore.value) {
				self.ignore.push(Resolved {
					value: pattern.value.anchored_at(contribution.anchor),
					origin: contribution.origin(pattern.position),
				});
			}
			for mapping in documents
				.adapters
				.iter()
				.flat_map(|adapters| &adapters.value)
			{
				self.adapters.push(Resolved {
					value: AdapterMapping {
						pattern: mapping.value.pattern.anchored_at(contribution.anchor),
						adapter: mapping.value.adapter,
					},
					origin: contribution.origin(mapping.position),
				});
			}
		}
		add_rules(&mut self.rules, contribution);
		for (name, definition) in
			file.series.iter().flat_map(|series| &series.value)
		{
			let series = Series {
				name: name.clone(),
				scope: contribution.scope.clone(),
				anchor: contribution.anchor.clone(),
				pattern: definition.value.pattern.clone(),
				version_kind: definition.value.version_kind,
				lifecycle: definition.value.lifecycle,
				schedule: definition.value.schedule.clone(),
				origin: contribution.origin(definition.position),
			};
			let same = self
				.series
				.iter_mut()
				.find(|known| known.scope == series.scope && known.name == series.name);
			match same {
				Some(known) => *known = series,
				None => self.series.push(series),
			}
		}
	}
}

/// Adds a contribution's rules, each with its origin.
fn add_rules(
	rules: &mut BTreeMap<String, Vec<Resolved<Rule>>>,
	contribution: &Contribution<'_>,
) {
	let added = contribution
		.file
		.rules
		.iter()
		.flat_map(|rules| &rules.value);
	for (context, list) in added {
		let entry = rules.entry(context.clone()).or_default();
		for rule in &list.value {
			entry.push(Resolved {
				value: rule.value.clone(),
				origin: contribution.origin(rule.position),
			});
		}
	}
}

/// The effective defaults of a remote owner's documents.
///
/// Per-entry overrides in the owner's lockfile come after these and belong to
/// the claims module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSettings {
	/// The owner, which names `.source-sentry/remotes/<owner>.lock`.
	pub owner: String,
	/// Classes, owners and the schedule.
	pub defaults: DocumentDefaults,
	/// Rules that the repository config and its imports add to contexts. A paired
	/// config cannot add rules.
	pub rules: BTreeMap<String, Vec<Resolved<Rule>>>,
}

/// Whether a directory is a root, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RootStatus {
	/// The directory config sets `root` to this value.
	Explicit(bool),
	/// No config sets `root`, and the directory holds a lockfile, which makes
	/// it a root by default.
	Lockfile,
	/// No config sets `root`, and the directory holds a directory config but
	/// no lockfile. A config alone makes a root only with `root: true`.
	DirectoryConfig,
	/// The directory holds neither a lockfile nor a directory config.
	NotRoot,
}

impl RootStatus {
	/// Whether the directory is a root.
	#[must_use]
	pub fn is_root(self) -> bool {
		match self {
			Self::Explicit(root) => root,
			Self::Lockfile => true,
			Self::DirectoryConfig | Self::NotRoot => false,
		}
	}
}

/// The built-in path selection, compiled once.
#[derive(Clone, Debug)]
pub(super) struct BuiltIn {
	include: Vec<PathPattern>,
	adapters: Vec<AdapterMapping>,
}

impl BuiltIn {
	/// Compiles [`BUILT_IN_DOCUMENT_TYPES`].
	pub(super) fn new() -> Self {
		let mut built_in = Self {
			include: Vec::new(),
			adapters: Vec::new(),
		};
		for &(source, adapter) in BUILT_IN_DOCUMENT_TYPES {
			#[expect(
				clippy::expect_used,
				reason = "the built-in patterns are constant, and the tests compile them"
			)]
			let pattern =
				PathPattern::compile(source, false).expect("valid built-in pattern");
			built_in.include.push(pattern.clone());
			built_in.adapters.push(AdapterMapping { pattern, adapter });
		}
		built_in
	}
}

/// One file's place in resolution order: a config, or a preset it imports.
struct Contribution<'a> {
	file: &'a Parsed,
	import_chain: &'a [RepoPath],
	scope: Scope,
	/// The directory its path patterns are anchored at.
	anchor: &'a RepoPath,
}

impl Contribution<'_> {
	/// The origin of a value this contribution writes at `position`.
	fn origin(&self, position: Position) -> Origin {
		Origin {
			file: Some(self.file.path.clone()),
			position: Some(position),
			import_chain: self.import_chain.to_vec(),
			scope: self.scope.clone(),
		}
	}
}

/// Appends a config's contributions: its expanded imports, then itself.
fn contribute<'a>(
	contributions: &mut Vec<Contribution<'a>>,
	node: &'a Node,
	scope: &Scope,
	anchor: &'a RepoPath,
) {
	for expanded in &node.imports {
		contributions.push(Contribution {
			file: &expanded.preset,
			import_chain: &expanded.chain,
			scope: scope.clone(),
			anchor,
		});
	}
	contributions.push(Contribution {
		file: &node.file,
		import_chain: &[],
		scope: scope.clone(),
		anchor,
	});
}

/// `directory` and every directory above it, from the repository root down.
fn lineage(directory: &RepoPath) -> Vec<RepoPath> {
	let mut lineage = vec![directory.clone()];
	let mut current = directory.parent();
	while let Some(parent) = current {
		current = parent.parent();
		lineage.push(parent);
	}
	lineage.reverse();
	lineage
}

impl ConfigSet {
	/// The snapshot these configs were read from, which completes every
	/// [`Origin`].
	#[must_use]
	pub fn revision(&self) -> &Revision {
		&self.revision
	}

	/// The repository's schema and default-policy version: the repository
	/// config's `version`, or [`IMPLICIT_VERSION`](super::IMPLICIT_VERSION)
	/// without one.
	#[must_use]
	pub fn version(&self) -> &Resolved<u32> {
		&self.version
	}

	/// The time zone for cadence dates, as written: the repository config's
	/// `time-zone`, or UTC. The schedule module validates the name.
	#[must_use]
	pub fn time_zone(&self) -> &Resolved<String> {
		&self.time_zone
	}

	/// The classes the repository config declares.
	#[must_use]
	pub fn classes(&self) -> &BTreeMap<ClassName, ClassDefinition> {
		&self.classes
	}

	/// The contexts the repository config declares. The built-in default context
	/// is not among them, unless the repository config replaces it by declaring
	/// [`DEFAULT_CONTEXT`](super::DEFAULT_CONTEXT).
	#[must_use]
	pub fn contexts(&self) -> &BTreeMap<String, ContextDefinition> {
		&self.contexts
	}

	/// Whether `name` is a context that may be selected and given rules: a
	/// declared one, or the built-in default.
	#[must_use]
	pub fn is_declared_context(&self, name: &str) -> bool {
		name == DEFAULT_CONTEXT || self.contexts.contains_key(name)
	}

	/// Every directory that holds a directory config.
	pub fn config_directories(&self) -> impl Iterator<Item = &RepoPath> {
		self.directories.keys()
	}

	/// Every directory that holds a lockfile, `source-sentry.lock`.
	pub fn lockfile_directories(&self) -> impl Iterator<Item = &RepoPath> {
		self.lockfile_directories.iter()
	}

	/// Every remote owner with a paired config.
	pub fn paired_owners(&self) -> impl Iterator<Item = &str> {
		self.remotes.keys().map(String::as_str)
	}

	/// The effective settings of `directory`, each with its origin.
	#[must_use]
	pub fn settings(&self, directory: &RepoPath) -> DirectorySettings {
		let mut settings = DirectorySettings::built_in(directory, &self.built_in);
		for contribution in self.contributions(directory) {
			settings.apply(&contribution);
		}
		settings.series.sort_by(|left, right| {
			(&left.scope, &left.name).cmp(&(&right.scope, &right.name))
		});
		settings
	}

	/// The effective defaults of a remote owner's documents: the repository
	/// config, then the paired config's imports, then its own settings. Without a
	/// paired config, the repository's defaults apply.
	#[must_use]
	pub fn remote_settings(&self, owner: &str) -> RemoteSettings {
		let mut contributions = Vec::new();
		if let Some(repository) = &self.repository {
			contribute(&mut contributions, repository, &Scope::Repository, &ROOT);
		}
		if let Some(paired) = self.remotes.get(owner) {
			contribute(
				&mut contributions,
				paired,
				&Scope::Remote(owner.to_owned()),
				&ROOT,
			);
		}
		let mut settings = RemoteSettings {
			owner: owner.to_owned(),
			defaults: DocumentDefaults::default(),
			rules: BTreeMap::new(),
		};
		for contribution in &contributions {
			settings.defaults.apply(contribution);
			add_rules(&mut settings.rules, contribution);
		}
		settings
	}

	/// The configs that reach `directory`, in resolution order: the repository
	/// config, then each directory config from the deepest `inherit: false` at or
	/// above `directory` down to `directory`, each after its imports.
	fn contributions(&self, directory: &RepoPath) -> Vec<Contribution<'_>> {
		let mut chain: Vec<(&RepoPath, &Node)> = lineage(directory)
			.iter()
			.filter_map(|ancestor| self.directories.get_key_value(ancestor))
			.collect();
		let cut = chain.iter().rposition(|(_, node)| {
			node
				.file
				.inherit
				.as_ref()
				.is_some_and(|inherit| !inherit.value)
		});
		if let Some(cut) = cut {
			chain.drain(..cut);
		}
		let mut contributions = Vec::new();
		if let Some(repository) = &self.repository {
			contribute(&mut contributions, repository, &Scope::Repository, &ROOT);
		}
		for (directory, node) in chain {
			contribute(
				&mut contributions,
				node,
				&Scope::Directory(directory.clone()),
				directory,
			);
		}
		contributions
	}

	/// Whether `directory` is a root, and why: the directory config's `root` when
	/// it sets one; otherwise, a directory that holds a lockfile is a root. A
	/// config alone makes a root only with `root: true`. `root` is never
	/// inherited.
	#[must_use]
	pub fn root_status(&self, directory: &RepoPath) -> RootStatus {
		let config = self.directories.get(directory);
		if let Some(root) = config.and_then(|node| node.file.root.as_ref()) {
			return RootStatus::Explicit(root.value);
		}
		if self.lockfile_directories.contains(directory) {
			RootStatus::Lockfile
		} else if config.is_some() {
			RootStatus::DirectoryConfig
		} else {
			RootStatus::NotRoot
		}
	}

	/// Whether `directory` is a root.
	#[must_use]
	pub fn is_root(&self, directory: &RepoPath) -> bool {
		self.root_status(directory).is_root()
	}

	/// Every root, sorted.
	#[must_use]
	pub fn roots(&self) -> Vec<RepoPath> {
		let mut roots: Vec<RepoPath> = self
			.directories
			.keys()
			.chain(&self.lockfile_directories)
			.filter(|directory| self.is_root(directory))
			.cloned()
			.collect();
		roots.sort();
		roots.dedup();
		roots
	}

	/// The root whose scan covers `path`: the nearest directory that contains
	/// `path` and is a root, since a scan stops where a nested root begins.
	/// `None` when no root contains `path`.
	#[must_use]
	pub fn root_of(&self, path: &RepoPath) -> Option<RepoPath> {
		let mut directory = path.parent();
		while let Some(current) = directory {
			if self.is_root(&current) {
				return Some(current);
			}
			directory = current.parent();
		}
		None
	}

	/// The roots nested directly in `root`: those below it with no other root in
	/// between. A scan of `root` stops at each of them.
	#[must_use]
	pub fn nested_roots(&self, root: &RepoPath) -> Vec<RepoPath> {
		self
			.roots()
			.into_iter()
			.filter(|nested| nested != root && nested.starts_with(root))
			.filter(|nested| {
				let mut between = nested.parent();
				while let Some(directory) = between {
					if &directory == root {
						return true;
					}
					if self.is_root(&directory) {
						return false;
					}
					between = directory.parent();
				}
				true
			})
			.collect()
	}

	/// Whether `path`, a file, is eligible as a document under its directory's
	/// settings.
	#[must_use]
	pub fn is_eligible(&self, path: &RepoPath) -> bool {
		let directory = path.parent().unwrap_or(RepoPath::ROOT);
		self.settings(&directory).eligibility(path).is_eligible()
	}

	/// The adapter for `path`, a file, under its directory's settings, or `None`
	/// when no mapping matches it.
	#[must_use]
	pub fn adapter(&self, path: &RepoPath) -> Option<Adapter> {
		let directory = path.parent().unwrap_or(RepoPath::ROOT);
		self
			.settings(&directory)
			.adapter(path)
			.map(|mapping| mapping.value.adapter)
	}
}
