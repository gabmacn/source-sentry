//! Explicit preset lookup and class and context definitions.

use super::{
	BTreeMap, BTreeSet, ClassDefinition, ClassName, ConfigError, ConfigErrorKind,
	ContextDefinition, Expanded, Files, Found, RepoPath, classes, contexts,
	discover, expand,
};

/// Adds enclosing projects' presets without replacing a nearer name.
pub(super) fn find_presets(
	found: &mut Found,
	paths: &[RepoPath],
	projects: &BTreeSet<RepoPath>,
	project: &RepoPath,
) {
	for parent in projects
		.iter()
		.rev()
		.filter(|parent| *parent != project && project.starts_with(parent))
	{
		let enclosing = discover(
			paths
				.iter()
				.filter(|path| crate::config::project_of(projects, path) == *parent)
				.cloned()
				.collect(),
			parent,
		);
		for (name, path) in enclosing.presets {
			found.presets.entry(name).or_insert(path);
		}
		for (name, path) in enclosing.misnamed_presets {
			found.misnamed_presets.entry(name).or_insert(path);
		}
	}
}

/// The project's explicit imports and own file, in resolution order.
pub(super) fn definitions(files: &Files) -> Vec<Expanded> {
	let Some(repository) = &files.repository else {
		return Vec::new();
	};
	let mut definitions = expand(repository, &files.presets, &mut Vec::new());
	definitions.push(Expanded {
		preset: std::sync::Arc::clone(repository),
		chain: Vec::new(),
	});
	definitions
}

/// Checks import sites and definitions before building infallible settings.
pub(super) fn validate(
	files: &Files,
	definitions: &[Expanded],
	errors: &mut Vec<ConfigError>,
) {
	for file in files.all().filter(|file| {
		files
			.repository
			.as_ref()
			.is_none_or(|repository| file.path != repository.path)
	}) {
		for (index, import) in file.imports.iter().enumerate() {
			if let Some(preset) = files.presets.get(&import.value)
				&& preset.define.is_some()
			{
				errors.push(
					ConfigError::new(
						&file.path,
						ConfigErrorKind::DefinitionImport {
							preset: preset.path.clone(),
						},
					)
					.at(Some(import.position))
					.in_setting(format!("imports[{index}]")),
				);
			}
		}
	}
	let mut seen = BTreeMap::new();
	for definition in definitions {
		let file = &definition.preset;
		let Some(define) = &file.define else {
			continue;
		};
		let names = define
			.classes
			.iter()
			.map(|(name, value)| ("class", name.to_string(), value.position))
			.chain(
				define
					.contexts
					.iter()
					.map(|(name, value)| ("context", name.clone(), value.position)),
			);
		for (kind, name, position) in names {
			if let Some(previous) =
				seen.insert((kind, name.clone()), file.path.clone())
			{
				errors.push(
					ConfigError::new(
						&file.path,
						ConfigErrorKind::DuplicateDefinition {
							kind,
							name: name.clone(),
							previous,
						},
					)
					.at(Some(position))
					.in_setting(format!(
						"define.{}.{name}",
						if kind == "class" {
							"classes"
						} else {
							"contexts"
						}
					)),
				);
			}
		}
	}
}

/// Merges checked class definitions, retaining their import chains.
pub(super) fn classes_from(
	definitions: &[Expanded],
) -> BTreeMap<ClassName, ClassDefinition> {
	let mut all = BTreeMap::new();
	for definition in definitions {
		for (name, mut class) in classes(Some(&definition.preset)) {
			class.origin.import_chain.clone_from(&definition.chain);
			all.insert(name, class);
		}
	}
	all
}

/// Merges checked context definitions, retaining origins for each rule.
pub(super) fn contexts_from(
	definitions: &[Expanded],
) -> BTreeMap<String, ContextDefinition> {
	let mut all = BTreeMap::new();
	for definition in definitions {
		for (name, mut context) in contexts(Some(&definition.preset)) {
			context.origin.import_chain.clone_from(&definition.chain);
			for origin in &mut context.rule_origins {
				origin.import_chain.clone_from(&definition.chain);
			}
			all.insert(name, context);
		}
	}
	all
}
