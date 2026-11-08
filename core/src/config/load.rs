//! Loading every config of a snapshot, and the checks that need more than one
//! file.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::error::{ConfigError, ConfigErrorKind, LoadError};
use super::parse::{self, ConfigKind, Parsed};
use super::pattern::quote;
use super::resolve::{
	BuiltIn, ClassDefinition, ContextDefinition, Origin, Resolved, Scope,
};
use super::{DEFAULT_CONTEXT, DEFAULT_TIME_ZONE, IMPLICIT_VERSION};
use crate::model::{ClassName, RepoPath};
use crate::ports::{BackendError, Revision, Snapshot};
use crate::{
	CONFIG_NAME, DIR_NAME, LOCKFILE_NAME, PRESETS_DIR_NAME, REMOTES_DIR_NAME,
};

/// Every config of one snapshot, parsed and validated together.
///
/// Built by [`ConfigSet::load`]. Its queries, such as
/// [`ConfigSet::settings`], cannot fail, because loading has already checked
/// everything they rely on.
#[derive(Clone, Debug)]
pub struct ConfigSet {
	/// The snapshot the configs were read from, which completes every [`Origin`].
	pub(super) revision: Revision,
	pub(super) version: Resolved<u32>,
	pub(super) time_zone: Resolved<String>,
	pub(super) classes: BTreeMap<ClassName, ClassDefinition>,
	pub(super) contexts: BTreeMap<String, ContextDefinition>,
	pub(super) repository: Option<Node>,
	/// Directory configs, by the directory that holds them.
	pub(super) directories: BTreeMap<RepoPath, Node>,
	/// Paired configs, by owner.
	pub(super) remotes: BTreeMap<String, Node>,
	pub(super) lockfile_directories: BTreeSet<RepoPath>,
	pub(super) built_in: BuiltIn,
}

/// A config with its imports expanded.
#[derive(Clone, Debug)]
pub(super) struct Node {
	pub(super) file: Arc<Parsed>,
	/// Every preset the config imports, depth-first in the order listed, with
	/// repetitions, each after the presets it imports itself.
	pub(super) imports: Vec<Expanded>,
}

/// One preset in a config's expanded imports.
#[derive(Clone, Debug)]
pub(super) struct Expanded {
	pub(super) preset: Arc<Parsed>,
	/// The config that imports it, then every preset on the way, outermost
	/// first.
	pub(super) chain: Vec<RepoPath>,
}

/// The config files and lockfiles among a snapshot's paths.
#[derive(Debug, Default)]
struct Found {
	repository: Option<RepoPath>,
	/// Presets, by import identifier.
	presets: BTreeMap<String, RepoPath>,
	/// `.yml` files in the presets folder, by the identifier they would have
	/// as `.yaml` files, for a correction.
	misnamed_presets: BTreeMap<String, RepoPath>,
	/// Paired configs, by owner.
	remotes: BTreeMap<String, RepoPath>,
	/// Directory configs, by the directory that holds them.
	directories: BTreeMap<RepoPath, RepoPath>,
	lockfile_directories: BTreeSet<RepoPath>,
}

/// Sorts the snapshot's paths into config files and lockfiles.
///
/// Nothing under `.source-sentry/` is a directory config or a local
/// lockfile: the repository config, presets and remote files live there.
fn discover(paths: Vec<RepoPath>) -> Found {
	let mut found = Found::default();
	for path in paths {
		if let Some(inside) = path
			.as_str()
			.strip_prefix(DIR_NAME)
			.and_then(|rest| rest.strip_prefix('/'))
		{
			discover_inside(&mut found, &path, inside);
			continue;
		}
		let Some(directory) = path.parent() else {
			continue;
		};
		match path.file_name() {
			Some(CONFIG_NAME) => {
				found.directories.insert(directory, path);
			}
			Some(LOCKFILE_NAME) => {
				found.lockfile_directories.insert(directory);
			}
			_ => {}
		}
	}
	found
}

/// Sorts a path inside `.source-sentry/`, given relative to it.
fn discover_inside(found: &mut Found, path: &RepoPath, inside: &str) {
	if inside == CONFIG_NAME {
		found.repository = Some(path.clone());
	} else if let Some(preset) = inside
		.strip_prefix(PRESETS_DIR_NAME)
		.and_then(|rest| rest.strip_prefix('/'))
	{
		if let Some(name) = file_stem(preset, ".yaml") {
			found.presets.insert(name.to_owned(), path.clone());
		} else if let Some(name) = file_stem(preset, ".yml") {
			found.misnamed_presets.insert(name.to_owned(), path.clone());
		}
	} else if let Some(remote) = inside
		.strip_prefix(REMOTES_DIR_NAME)
		.and_then(|rest| rest.strip_prefix('/'))
		&& !remote.contains('/')
		&& let Some(owner) = file_stem(remote, ".yaml")
	{
		found.remotes.insert(owner.to_owned(), path.clone());
	}
}

/// `path` without `extension`, when it has the extension and a non-empty
/// file name before it.
fn file_stem<'p>(path: &'p str, extension: &str) -> Option<&'p str> {
	path
		.strip_suffix(extension)
		.filter(|stem| !stem.is_empty() && !stem.ends_with('/'))
}

/// The files parsed from a snapshot. A file that failed to parse is absent,
/// and its errors are already recorded.
#[derive(Debug, Default)]
struct Files {
	repository: Option<Arc<Parsed>>,
	presets: BTreeMap<String, Arc<Parsed>>,
	directories: BTreeMap<RepoPath, Arc<Parsed>>,
	remotes: BTreeMap<String, Arc<Parsed>>,
}

impl Files {
	/// Reads and parses every config that `found` lists.
	fn read<S: Snapshot>(
		snapshot: &S,
		found: &Found,
		errors: &mut Vec<ConfigError>,
	) -> Result<Self, BackendError> {
		let mut read = |path: &RepoPath, kind| -> Result<_, BackendError> {
			let bytes = snapshot.read(path)?;
			Ok(match parse::parse(path, kind, &bytes) {
				Ok(parsed) => Some(Arc::new(parsed)),
				Err(file_errors) => {
					errors.extend(file_errors);
					None
				}
			})
		};
		let mut files = Self::default();
		if let Some(path) = &found.repository {
			files.repository = read(path, ConfigKind::Repository)?;
		}
		for (name, path) in &found.presets {
			if let Some(parsed) = read(path, ConfigKind::Preset)? {
				files.presets.insert(name.clone(), parsed);
			}
		}
		for (directory, path) in &found.directories {
			if let Some(parsed) = read(path, ConfigKind::Directory)? {
				files.directories.insert(directory.clone(), parsed);
			}
		}
		for (owner, path) in &found.remotes {
			if let Some(parsed) = read(path, ConfigKind::Paired)? {
				files.remotes.insert(owner.clone(), parsed);
			}
		}
		Ok(files)
	}

	/// Every parsed file.
	fn all(&self) -> impl Iterator<Item = &Arc<Parsed>> {
		self
			.repository
			.iter()
			.chain(self.presets.values())
			.chain(self.directories.values())
			.chain(self.remotes.values())
	}
}

impl ConfigSet {
	/// Reads and validates every config of `snapshot`.
	///
	/// Directory configs are found among all of the snapshot's paths, whatever
	/// the ignore patterns say. Without a repository config, the repository uses
	/// [`IMPLICIT_VERSION`](super::IMPLICIT_VERSION), UTC, no declared classes,
	/// and only the built-in [`DEFAULT_CONTEXT`](super::DEFAULT_CONTEXT).
	///
	/// # Errors
	///
	/// - [`LoadError::Backend`] when the snapshot cannot list or read its
	///   files.
	/// - [`LoadError::Invalid`] with every configuration error found: each
	///   file's own mistakes (see the [module documentation](super)), classes
	///   and contexts that the repository config does not declare, unknown
	///   presets, import cycles, and paired configs that import presets with
	///   settings a paired config may not have.
	pub fn load<S: Snapshot>(snapshot: &S) -> Result<Self, LoadError> {
		let found = discover(snapshot.paths()?);
		let mut errors = Vec::new();
		let files = Files::read(snapshot, &found, &mut errors)?;

		// Without a valid repository config, declarations are unknown, and
		// checking against them would only repeat its own errors.
		if found.repository.is_none() || files.repository.is_some() {
			let declarations = Declarations::of(files.repository.as_deref());
			for file in files.all() {
				declarations.check(file, &mut errors);
			}
		}
		check_imports(&files, &found, &mut errors);

		let node = |file: &Arc<Parsed>| Node {
			file: Arc::clone(file),
			imports: expand(file, &files.presets, &mut Vec::new()),
		};
		let repository = files.repository.as_ref().map(node);
		let directories: BTreeMap<_, _> = files
			.directories
			.iter()
			.map(|(directory, file)| (directory.clone(), node(file)))
			.collect();
		let remotes: BTreeMap<_, _> = files
			.remotes
			.iter()
			.map(|(owner, file)| (owner.clone(), node(file)))
			.collect();
		for remote in remotes.values() {
			check_paired_imports(remote, &mut errors);
		}

		if !errors.is_empty() {
			errors.sort_by(|left, right| {
				(&left.file, left.position).cmp(&(&right.file, right.position))
			});
			errors.dedup();
			return Err(LoadError::Invalid(errors));
		}
		let repository_file = repository.as_ref().map(|node| node.file.as_ref());
		Ok(Self {
			revision: snapshot.revision().clone(),
			version: version(repository_file),
			time_zone: time_zone(repository_file),
			classes: classes(repository_file),
			contexts: contexts(repository_file),
			repository,
			directories,
			remotes,
			lockfile_directories: found.lockfile_directories,
			built_in: BuiltIn::new(),
		})
	}
}

/// The classes and contexts that configs may use.
struct Declarations {
	classes: BTreeSet<ClassName>,
	contexts: BTreeSet<String>,
	/// Whether a repository config exists, for corrections.
	has_repository: bool,
}

impl Declarations {
	/// The repository config's declarations, plus the built-in default
	/// context.
	fn of(repository: Option<&Parsed>) -> Self {
		let define = repository.and_then(|file| file.define.as_ref());
		let mut contexts: BTreeSet<String> = define
			.map(|define| define.contexts.keys().cloned().collect())
			.unwrap_or_default();
		contexts.insert(DEFAULT_CONTEXT.to_owned());
		Self {
			classes: define
				.map(|define| define.classes.keys().cloned().collect())
				.unwrap_or_default(),
			contexts,
			has_repository: repository.is_some(),
		}
	}

	/// Where to declare a name, for a correction.
	fn correction(&self, key: &str) -> String {
		if self.has_repository {
			format!(
				"Declare it under `define.{key}` in .source-sentry/source-sentry.yaml"
			)
		} else {
			format!(
				"Create .source-sentry/source-sentry.yaml and declare it under \
				 `define.{key}`"
			)
		}
	}

	/// Reports every class and context that `file` uses without a
	/// declaration.
	fn check(&self, file: &Parsed, errors: &mut Vec<ConfigError>) {
		let undeclared_class = |class: &ClassName, setting: String, position| {
			ConfigError::new(
				&file.path,
				ConfigErrorKind::UndeclaredClass {
					class: class.to_string(),
				},
			)
			.at(Some(position))
			.in_setting(setting)
			.correct(self.correction("classes"))
		};
		if let Some(classes) = &file.classes {
			for (index, class) in classes.value.iter().enumerate() {
				if !self.classes.contains(&class.value) {
					errors.push(undeclared_class(
						&class.value,
						format!("classes[{index}]"),
						class.position,
					));
				}
			}
		}
		for (context, list) in file.rules.iter().flat_map(|rules| &rules.value) {
			if !self.contexts.contains(context) {
				errors.push(
					ConfigError::new(
						&file.path,
						ConfigErrorKind::UndeclaredContext {
							context: context.clone(),
						},
					)
					.at(Some(list.position))
					.in_setting(format!("rules.{context}"))
					.correct(self.correction("contexts")),
				);
			}
			for (index, rule) in list.value.iter().enumerate() {
				for class in &rule.value.classes {
					if !self.classes.contains(class) {
						let setting = format!("rules.{context}[{index}].classes");
						errors.push(undeclared_class(class, setting, rule.position));
					}
				}
			}
		}
		let contexts = file.define.iter().flat_map(|define| &define.contexts);
		for (name, context) in contexts {
			for (index, rule) in context.value.rules.iter().enumerate() {
				for class in &rule.classes {
					if !self.classes.contains(class) {
						let setting =
							format!("define.contexts.{name}.rules[{index}].classes");
						errors.push(undeclared_class(class, setting, context.position));
					}
				}
			}
		}
	}
}

/// Reports imports of presets that do not exist, and import cycles.
fn check_imports(files: &Files, found: &Found, errors: &mut Vec<ConfigError>) {
	let available = found
		.presets
		.keys()
		.map(|name| quote(name))
		.collect::<Vec<_>>()
		.join(", ");
	for file in files.all() {
		for (index, import) in file.imports.iter().enumerate() {
			if found.presets.contains_key(&import.value) {
				continue;
			}
			let correction =
				if let Some(misnamed) = found.misnamed_presets.get(&import.value) {
					format!("Rename {misnamed} to end in `.yaml`")
				} else if available.is_empty() {
					format!(
						"Add .source-sentry/presets/{}.yaml, or remove the import",
						import.value
					)
				} else {
					format!("The presets are {available}")
				};
			errors.push(
				ConfigError::new(
					&file.path,
					ConfigErrorKind::UnknownPreset {
						name: import.value.clone(),
					},
				)
				.at(Some(import.position))
				.in_setting(format!("imports[{index}]"))
				.correct(correction),
			);
		}
	}
	let mut finished = BTreeSet::new();
	for name in files.presets.keys() {
		find_cycles(name, &files.presets, &mut Vec::new(), &mut finished, errors);
	}
}

/// Walks the imports of the preset `name` depth-first, reporting every
/// import that closes a cycle on the active stack.
fn find_cycles<'p>(
	name: &'p str,
	presets: &'p BTreeMap<String, Arc<Parsed>>,
	active: &mut Vec<&'p str>,
	finished: &mut BTreeSet<&'p str>,
	errors: &mut Vec<ConfigError>,
) {
	let Some(preset) = presets.get(name) else {
		return;
	};
	if finished.contains(name) {
		return;
	}
	active.push(name);
	for (index, import) in preset.imports.iter().enumerate() {
		let imported = import.value.as_str();
		if let Some(start) = active.iter().position(|open| *open == imported) {
			let mut cycle: Vec<String> = active[start..]
				.iter()
				.map(|open| (*open).to_owned())
				.collect();
			cycle.push(imported.to_owned());
			errors.push(
				ConfigError::new(&preset.path, ConfigErrorKind::ImportCycle { cycle })
					.at(Some(import.position))
					.in_setting(format!("imports[{index}]"))
					.correct("Remove one of the imports on the cycle"),
			);
		} else {
			find_cycles(imported, presets, active, finished, errors);
		}
	}
	active.pop();
	finished.insert(name);
}

/// Expands `file`'s imports depth-first, in the order listed, keeping
/// repetitions. `active` holds the presets being expanded.
///
/// Unknown presets and imports that close a cycle are skipped here, because
/// [`check_imports`] reports them and loading then fails.
fn expand(
	file: &Parsed,
	presets: &BTreeMap<String, Arc<Parsed>>,
	active: &mut Vec<String>,
) -> Vec<Expanded> {
	let mut expanded = Vec::new();
	for import in &file.imports {
		let Some(preset) = presets.get(&import.value) else {
			continue;
		};
		if active.contains(&import.value) {
			continue;
		}
		active.push(import.value.clone());
		for inner in expand(preset, presets, active) {
			let mut chain = Vec::with_capacity(inner.chain.len() + 1);
			chain.push(file.path.clone());
			chain.extend(inner.chain);
			expanded.push(Expanded {
				preset: inner.preset,
				chain,
			});
		}
		expanded.push(Expanded {
			preset: Arc::clone(preset),
			chain: vec![file.path.clone()],
		});
		active.pop();
	}
	expanded
}

/// Reports settings that a paired config may not have, but brings in through
/// its imports. A preset's `root`, `inherit`, `version` and `define` are
/// already errors on their own.
fn check_paired_imports(remote: &Node, errors: &mut Vec<ConfigError>) {
	for expanded in &remote.imports {
		let preset = &expanded.preset;
		let forbidden = [
			(
				"documents",
				preset.documents.as_ref().map(|value| value.position),
			),
			("rules", preset.rules.as_ref().map(|value| value.position)),
			("series", preset.series.as_ref().map(|value| value.position)),
		];
		for (key, position) in forbidden {
			let Some(position) = position else {
				continue;
			};
			errors.push(
				ConfigError::new(
					&preset.path,
					ConfigErrorKind::ForbiddenSetting {
						key: key.to_owned(),
						kind: ConfigKind::Paired,
					},
				)
				.at(Some(position))
				.in_setting(key)
				.through(expanded.chain.clone())
				.correct(format!(
					"A paired config allows only `imports`, `classes`, `owners` and \
					 `schedule`, including through its imports; remove `{key}` from \
					 this preset, or stop importing it from {}",
					remote.file.path
				)),
			);
		}
	}
}

/// The origin of a value that the repository config sets at `position`.
fn repository_origin(file: &Parsed, position: super::Position) -> Origin {
	Origin {
		file: Some(file.path.clone()),
		position: Some(position),
		import_chain: Vec::new(),
		scope: Scope::Repository,
	}
}

/// The repository's version, or the implicit one without a repository config.
/// Loading has checked that a repository config sets a supported version.
fn version(repository: Option<&Parsed>) -> Resolved<u32> {
	match repository
		.and_then(|file| file.version.as_ref().map(|version| (file, version)))
	{
		Some((file, version)) => Resolved {
			value: version.value,
			origin: repository_origin(file, version.position),
		},
		None => Resolved {
			value: IMPLICIT_VERSION,
			origin: Origin::built_in(),
		},
	}
}

/// The time zone for cadence dates.
fn time_zone(repository: Option<&Parsed>) -> Resolved<String> {
	let configured = repository
		.and_then(|file| file.time_zone.as_ref().map(|zone| (file, zone)));
	match configured {
		Some((file, zone)) => Resolved {
			value: zone.value.clone(),
			origin: repository_origin(file, zone.position),
		},
		None => Resolved {
			value: DEFAULT_TIME_ZONE.to_owned(),
			origin: Origin::built_in(),
		},
	}
}

/// The declared classes.
fn classes(
	repository: Option<&Parsed>,
) -> BTreeMap<ClassName, ClassDefinition> {
	let Some((file, define)) = repository
		.and_then(|file| file.define.as_ref().map(|define| (file, define)))
	else {
		return BTreeMap::new();
	};
	define
		.classes
		.iter()
		.map(|(name, class)| {
			(
				name.clone(),
				ClassDefinition {
					description: class.value.clone(),
					origin: repository_origin(file, class.position),
				},
			)
		})
		.collect()
}

/// The declared contexts.
fn contexts(
	repository: Option<&Parsed>,
) -> BTreeMap<String, ContextDefinition> {
	let Some((file, define)) = repository
		.and_then(|file| file.define.as_ref().map(|define| (file, define)))
	else {
		return BTreeMap::new();
	};
	define
		.contexts
		.iter()
		.map(|(name, context)| {
			(
				name.clone(),
				ContextDefinition {
					context: context.value.clone(),
					origin: repository_origin(file, context.position),
				},
			)
		})
		.collect()
}
