//! Tests for contexts, judgements and gates.

use std::collections::{BTreeMap, BTreeSet};

use super::defaults::parse_context;
use super::*;
use crate::model::{ClassName, Context, Fact, Outcome, Rule};

/// Every fact, in declaration order.
const ALL_FACTS: [Fact; 9] = [
	Fact::Fresh,
	Fact::Stale,
	Fact::Overdue,
	Fact::Unattested,
	Fact::Unclaimed,
	Fact::UpstreamStale,
	Fact::TimingUnknown,
	Fact::Missed,
	Fact::AwaitingSeal,
];

fn class(name: &str) -> ClassName {
	ClassName::new(name).unwrap()
}

fn classes(names: &[&str]) -> Vec<ClassName> {
	names.iter().map(|name| class(name)).collect()
}

fn declared_classes(names: &[&str]) -> BTreeSet<ClassName> {
	names.iter().map(|name| class(name)).collect()
}

fn rule(classes_: &[&str], facts: &[Fact], outcome: Outcome) -> Rule {
	Rule {
		classes: classes(classes_),
		facts: facts.to_vec(),
		outcome,
	}
}

fn context(yaml: &str) -> Context {
	parse_context(yaml).unwrap()
}

/// The `production` context of `structure/repository-config.example.yaml`.
const PRODUCTION: &str = "\
fallback: warn
rules:
  - facts: ['missed']
    outcome: notice
  - classes: ['privacy']
    facts: ['stale', 'overdue', 'unattested', 'timing-unknown', 'upstream-stale']
    outcome: fail
";

/// The contexts of `structure/repository-config.example.yaml`.
fn example_contexts() -> BTreeMap<String, Context> {
	let default = "\
fallback: warn
rules:
  - facts: ['missed', 'unattested']
    outcome: notice
";
	let staging = "\
fallback: warn
rules:
  - facts: ['missed']
    outcome: notice
";
	BTreeMap::from([
		("default".to_owned(), context(default)),
		("production".to_owned(), context(PRODUCTION)),
		("staging".to_owned(), context(staging)),
	])
}

fn outcome_of(judgement: &Judgement, fact: Fact) -> Outcome {
	judgement
		.facts
		.iter()
		.find(|judged| judged.fact == fact)
		.unwrap()
		.outcome
}

fn judged(judgement: &Judgement, fact: Fact) -> &FactJudgement {
	judgement
		.facts
		.iter()
		.find(|judged| judged.fact == fact)
		.unwrap()
}

// The built-in default context.

#[test]
fn built_in_v1_is_the_intended_context() {
	let expected = Context {
		fallback: Outcome::Warn,
		rules: vec![rule(&[], &[Fact::Missed], Outcome::Notice)],
	};
	assert_eq!(default_context(1).unwrap(), expected);
}

#[test]
fn every_shipped_version_parses_and_validates() {
	for version in DEFAULTS_VERSIONS {
		assert!(default_context_source(version).is_some());
		let context = default_context(version).unwrap();
		validate_context("default", &context, &BTreeSet::new()).unwrap();
	}
}

#[test]
fn unsupported_defaults_versions_are_configuration_errors() {
	for version in [0, 2, u32::MAX] {
		assert_eq!(default_context_source(version), None);
		let error = default_context(version).unwrap_err();
		assert_eq!(error, PolicyError::UnsupportedDefaultsVersion { version });
		assert_eq!(error.kind(), ErrorKind::Configuration);
		assert!(error.to_string().contains("this build supports 1"));
	}
}

#[test]
fn built_in_v1_passes_fresh_notes_missed_and_warns_on_the_rest() {
	let context = default_context(1).unwrap();
	for fact in ALL_FACTS {
		let expected = match fact {
			Fact::Fresh => Outcome::Pass,
			Fact::Missed => Outcome::Notice,
			_ => Outcome::Warn,
		};
		let judgement = judge(&context, &[], &[], [fact]);
		assert_eq!(judgement.outcome, expected, "{fact}");
	}
}

#[test]
fn contexts_are_parsed_strictly() {
	// Each of these would parse under serde-saphyr's default options, which
	// read the tagged class as plain `privacy`.
	let unsupported_tag = "\
fallback: warn
rules:
  - classes: [!custom 'privacy']
    facts: ['stale']
    outcome: fail
";
	let merge_key = "\
fallback: warn
rules:
  - &missed
    facts: ['missed']
    outcome: notice
  - <<: *missed
    outcome: warn
";
	for yaml in [unsupported_tag, merge_key] {
		assert!(serde_saphyr::from_str::<Context>(yaml).is_ok(), "{yaml}");
		assert!(parse_context(yaml).is_err(), "{yaml}");
	}
	// These are rejected by the defaults that the strict options keep.
	let duplicate_key = "fallback: warn\nfallback: fail\n";
	let two_documents = "fallback: warn\n---\nfallback: fail\n";
	for yaml in [duplicate_key, two_documents] {
		assert!(parse_context(yaml).is_err(), "{yaml}");
	}
}

// Selecting a context.

#[test]
fn no_name_selects_the_repository_default_when_declared() {
	let declared = example_contexts();
	let selected = select_context(None, &declared, 1).unwrap();
	assert_eq!(selected.name, "default");
	assert_eq!(selected.origin, ContextOrigin::Repository);
	assert_eq!(selected.context, declared["default"]);
	// The replacement is entire: the built-in context contributes nothing.
	let judgement = judge(&selected.context, &[], &[], [Fact::Unattested]);
	assert_eq!(judgement.outcome, Outcome::Notice);
}

#[test]
fn no_name_selects_the_built_in_default_otherwise() {
	let mut declared = example_contexts();
	declared.remove("default");
	let selected = select_context(None, &declared, 1).unwrap();
	assert_eq!(selected.name, DEFAULT_CONTEXT_NAME);
	assert_eq!(selected.origin, ContextOrigin::BuiltIn { version: 1 });
	assert_eq!(selected.context, default_context(1).unwrap());
}

#[test]
fn the_name_default_selects_like_no_name() {
	let declared = example_contexts();
	assert_eq!(
		select_context(Some("default"), &declared, 1).unwrap(),
		select_context(None, &declared, 1).unwrap(),
	);
	let selected = select_context(Some("default"), &BTreeMap::new(), 1).unwrap();
	assert_eq!(selected.origin, ContextOrigin::BuiltIn { version: 1 });
}

#[test]
fn a_declared_name_selects_that_context() {
	let declared = example_contexts();
	let selected = select_context(Some("production"), &declared, 1).unwrap();
	assert_eq!(selected.name, "production");
	assert_eq!(selected.origin, ContextOrigin::Repository);
	assert_eq!(selected.context, declared["production"]);
}

#[test]
fn an_unknown_name_is_an_error_not_the_default() {
	let mut declared = example_contexts();
	let error = select_context(Some("prodution"), &declared, 1).unwrap_err();
	assert_eq!(
		error,
		PolicyError::UnknownContext {
			name: "prodution".to_owned(),
			available: vec![
				"default".to_owned(),
				"production".to_owned(),
				"staging".to_owned(),
			],
		}
	);
	assert_eq!(error.kind(), ErrorKind::Configuration);
	assert!(error.to_string().contains("`prodution`"));

	// Without a repository default, the built-in one is still listed.
	declared.remove("default");
	let Err(PolicyError::UnknownContext { available, .. }) =
		select_context(Some("prodution"), &declared, 1)
	else {
		panic!("expected an unknown context");
	};
	assert_eq!(available, ["default", "production", "staging"]);
}

#[test]
fn an_unsupported_defaults_version_is_rejected_for_any_context() {
	let declared = example_contexts();
	for requested in [None, Some("production")] {
		assert_eq!(
			select_context(requested, &declared, 2).unwrap_err(),
			PolicyError::UnsupportedDefaultsVersion { version: 2 }
		);
	}
}

// Validating contexts.

#[test]
fn the_example_contexts_are_valid() {
	let declared =
		declared_classes(&["internal-docs", "marketing", "privacy", "security"]);
	for (name, context) in example_contexts() {
		validate_context(&name, &context, &declared).unwrap();
	}
}

#[test]
fn a_rule_targeting_fresh_is_an_error() {
	let rules = [rule(&[], &[Fact::Stale, Fact::Fresh], Outcome::Fail)];
	assert_eq!(
		validate_rules(&rules, &BTreeSet::new()).unwrap_err(),
		[RuleError {
			index: 0,
			problem: RuleProblem::TargetsFresh,
		}]
	);
}

#[test]
fn an_undeclared_class_is_an_error() {
	let declared = declared_classes(&["privacy"]);
	let rules = [rule(&["privacy", "legal"], &[Fact::Stale], Outcome::Fail)];
	assert_eq!(
		validate_rules(&rules, &declared).unwrap_err(),
		[RuleError {
			index: 0,
			problem: RuleProblem::UndeclaredClass(class("legal")),
		}]
	);
}

#[test]
fn an_empty_facts_list_is_an_error() {
	let rules = [rule(&[], &[], Outcome::Warn)];
	assert_eq!(
		validate_rules(&rules, &BTreeSet::new()).unwrap_err(),
		[RuleError {
			index: 0,
			problem: RuleProblem::NoFacts,
		}]
	);
}

#[test]
fn every_problem_is_reported_with_its_context() {
	let declared = declared_classes(&["privacy"]);
	let context = Context {
		fallback: Outcome::Warn,
		rules: vec![
			rule(&["privacy"], &[Fact::Stale], Outcome::Fail),
			rule(&["legal"], &[Fact::Fresh], Outcome::Fail),
			rule(&[], &[], Outcome::Notice),
		],
	};
	let error = validate_context("production", &context, &declared).unwrap_err();
	assert_eq!(
		error,
		PolicyError::InvalidContext {
			name: "production".to_owned(),
			errors: vec![
				RuleError {
					index: 1,
					problem: RuleProblem::TargetsFresh,
				},
				RuleError {
					index: 1,
					problem: RuleProblem::UndeclaredClass(class("legal")),
				},
				RuleError {
					index: 2,
					problem: RuleProblem::NoFacts,
				},
			],
		}
	);
	assert_eq!(error.kind(), ErrorKind::Configuration);
	let message = error.to_string();
	assert!(message.starts_with("context `production` is invalid: rule 2 "));
	assert!(message.contains("rule 3 has an empty `facts` list"));
}

// Judging documents.

#[test]
fn production_fails_a_stale_privacy_policy() {
	let production = context(PRODUCTION);
	let judgement =
		judge(&production, &[], &classes(&["privacy"]), [Fact::Stale]);
	assert_eq!(judgement.outcome, Outcome::Fail);
	assert_eq!(
		judged(&judgement, Fact::Stale).basis,
		Basis::Rule(RuleRef::Context(1))
	);
}

#[test]
fn a_broader_overdue_rule_cannot_weaken_a_stale_overdue_privacy_policy() {
	// The privacy rule fails only on `stale`; the broader rule warns on
	// `overdue` for every class.
	let narrow = Context {
		fallback: Outcome::Notice,
		rules: vec![
			rule(&["privacy"], &[Fact::Stale], Outcome::Fail),
			rule(&[], &[Fact::Overdue], Outcome::Warn),
		],
	};
	let privacy = classes(&["privacy"]);
	let judgement = judge(&narrow, &[], &privacy, [Fact::Stale, Fact::Overdue]);
	assert_eq!(judgement.outcome, Outcome::Fail);
	assert_eq!(outcome_of(&judgement, Fact::Stale), Outcome::Fail);
	assert_eq!(outcome_of(&judgement, Fact::Overdue), Outcome::Warn);

	// With the example's production context plus the broader rule, `overdue`
	// matches both rules and the more severe one decides.
	let mut production = context(PRODUCTION);
	production
		.rules
		.push(rule(&[], &[Fact::Overdue], Outcome::Warn));
	let judgement =
		judge(&production, &[], &privacy, [Fact::Stale, Fact::Overdue]);
	assert_eq!(judgement.outcome, Outcome::Fail);
	let overdue = judged(&judgement, Fact::Overdue);
	assert_eq!(overdue.outcome, Outcome::Fail);
	assert_eq!(overdue.basis, Basis::Rule(RuleRef::Context(1)));
	assert_eq!(overdue.matching, [RuleRef::Context(1), RuleRef::Context(2)]);
}

#[test]
fn production_only_warns_on_other_classes() {
	let production = context(PRODUCTION);
	let judgement = judge(
		&production,
		&[],
		&classes(&["internal-docs"]),
		[Fact::Stale, Fact::Overdue],
	);
	assert_eq!(judgement.outcome, Outcome::Warn);
	assert_eq!(judged(&judgement, Fact::Stale).basis, Basis::Fallback);
}

#[test]
fn scoped_rules_combine_with_context_rules() {
	let production = Context {
		fallback: Outcome::Warn,
		rules: vec![rule(&["privacy"], &[Fact::Stale], Outcome::Fail)],
	};
	let scoped = [
		rule(&["internal-docs"], &[Fact::Overdue], Outcome::Fail),
		rule(&[], &[Fact::Stale], Outcome::Notice),
	];

	// A scoped rule raises a fact above the context's fallback.
	let docs = classes(&["internal-docs"]);
	let judgement = judge(&production, &scoped, &docs, [Fact::Overdue]);
	assert_eq!(judgement.outcome, Outcome::Fail);
	assert_eq!(
		judged(&judgement, Fact::Overdue).basis,
		Basis::Rule(RuleRef::Scoped(0))
	);

	// A scoped rule cannot weaken a matching context rule.
	let both = classes(&["internal-docs", "privacy"]);
	let judgement = judge(&production, &scoped, &both, [Fact::Stale]);
	let stale = judged(&judgement, Fact::Stale);
	assert_eq!(stale.outcome, Outcome::Fail);
	assert_eq!(stale.basis, Basis::Rule(RuleRef::Context(0)));
	assert_eq!(stale.matching, [RuleRef::Context(0), RuleRef::Scoped(1)]);

	// A matching scoped rule takes the place of the fallback.
	let judgement = judge(&production, &scoped, &docs, [Fact::Stale]);
	assert_eq!(judgement.outcome, Outcome::Notice);
	assert_eq!(
		judged(&judgement, Fact::Stale).basis,
		Basis::Rule(RuleRef::Scoped(1))
	);
}

#[test]
fn equally_severe_rules_are_explained_by_the_first() {
	let context = Context {
		fallback: Outcome::Pass,
		rules: vec![rule(&[], &[Fact::Overdue], Outcome::Warn)],
	};
	let scoped = [rule(&[], &[Fact::Overdue], Outcome::Warn)];
	let judgement = judge(&context, &scoped, &[], [Fact::Overdue]);
	let overdue = judged(&judgement, Fact::Overdue);
	assert_eq!(overdue.basis, Basis::Rule(RuleRef::Context(0)));
	assert_eq!(overdue.matching, [RuleRef::Context(0), RuleRef::Scoped(0)]);
}

#[test]
fn unmatched_facts_take_the_fallback() {
	for fallback in [Outcome::Pass, Outcome::Notice, Outcome::Warn, Outcome::Fail]
	{
		let context = Context {
			fallback,
			rules: vec![
				rule(&[], &[Fact::Missed], Outcome::Notice),
				rule(&["privacy"], &[Fact::Unclaimed], Outcome::Fail),
			],
		};
		let judgement = judge(
			&context,
			&[],
			&classes(&["marketing"]),
			[Fact::Unclaimed, Fact::TimingUnknown],
		);
		assert_eq!(judgement.outcome, fallback);
		for judged in &judgement.facts {
			assert_eq!(judged.outcome, fallback);
			assert_eq!(judged.basis, Basis::Fallback);
			assert_eq!(judged.matching, Vec::<RuleRef>::new());
		}
	}
}

#[test]
fn class_filters_need_one_shared_class() {
	let context = Context {
		fallback: Outcome::Pass,
		rules: vec![rule(
			&["privacy", "security"],
			&[Fact::Stale],
			Outcome::Fail,
		)],
	};
	let outcome = |names: &[&str]| {
		judge(&context, &[], &classes(names), [Fact::Stale]).outcome
	};
	assert_eq!(outcome(&[]), Outcome::Pass);
	assert_eq!(outcome(&["marketing"]), Outcome::Pass);
	assert_eq!(outcome(&["marketing", "security"]), Outcome::Fail);
}

#[test]
fn fresh_alone_passes() {
	let strict = Context {
		fallback: Outcome::Fail,
		rules: Vec::new(),
	};
	let judgement = judge(&strict, &[], &[], [Fact::Fresh]);
	assert_eq!(judgement.outcome, Outcome::Pass);
	assert_eq!(
		judgement.facts,
		[FactJudgement {
			fact: Fact::Fresh,
			outcome: Outcome::Pass,
			basis: Basis::Fresh,
			matching: Vec::new(),
		}]
	);
}

#[test]
fn a_document_without_facts_passes() {
	let strict = Context {
		fallback: Outcome::Fail,
		rules: Vec::new(),
	};
	let judgement = judge(&strict, &[], &[], []);
	assert_eq!(judgement.outcome, Outcome::Pass);
	assert_eq!(judgement.facts, Vec::<FactJudgement>::new());
}

#[test]
fn no_rule_can_change_fresh() {
	// Validation rejects this rule; judging ignores it all the same.
	let invalid = Context {
		fallback: Outcome::Fail,
		rules: vec![rule(&[], &[Fact::Fresh], Outcome::Fail)],
	};
	let scoped = [rule(&[], &[Fact::Fresh], Outcome::Fail)];
	let judgement = judge(&invalid, &scoped, &[], [Fact::Fresh]);
	assert_eq!(judgement.outcome, Outcome::Pass);
	assert_eq!(judged(&judgement, Fact::Fresh).basis, Basis::Fresh);
}

#[test]
fn the_most_severe_fact_decides_and_repeats_are_judged_once() {
	let context = default_context(1).unwrap();
	let judgement = judge(
		&context,
		&[],
		&[],
		[Fact::Missed, Fact::Fresh, Fact::Stale, Fact::Missed],
	);
	assert_eq!(judgement.outcome, Outcome::Warn);
	let facts: Vec<Fact> =
		judgement.facts.iter().map(|judged| judged.fact).collect();
	assert_eq!(facts, [Fact::Fresh, Fact::Stale, Fact::Missed]);
}

// Gates and exit codes.

fn summary_of(outcomes: &[Outcome], errors: &[ErrorKind]) -> Summary {
	let mut summary = Summary::new();
	for &outcome in outcomes {
		summary.record_outcome(outcome);
	}
	for &kind in errors {
		summary.record_error(kind);
	}
	summary
}

#[test]
fn exit_codes_for_both_thresholds() {
	use ErrorKind::{Configuration, Integrity, Operational};
	use ExitCode::{
		BelowThreshold, ConfigurationOrIntegrity, Failure, Operational as Op,
		Warning,
	};
	use Outcome::{Fail, Notice, Pass, Warn};

	// (outcomes, errors, exit under `warn`, exit under `fail`)
	let cases: [(&[Outcome], &[ErrorKind], ExitCode, ExitCode); 11] = [
		(&[], &[], BelowThreshold, BelowThreshold),
		(&[Pass], &[], BelowThreshold, BelowThreshold),
		(&[Pass, Notice], &[], BelowThreshold, BelowThreshold),
		(&[Notice, Warn], &[], Warning, BelowThreshold),
		(&[Warn, Fail], &[], Failure, Failure),
		(&[Fail], &[], Failure, Failure),
		(
			&[Pass],
			&[Configuration],
			ConfigurationOrIntegrity,
			ConfigurationOrIntegrity,
		),
		(
			&[Fail],
			&[Integrity],
			ConfigurationOrIntegrity,
			ConfigurationOrIntegrity,
		),
		(&[], &[Operational], Op, Op),
		(&[Warn], &[Operational], Op, Op),
		(&[Fail], &[Configuration, Integrity, Operational], Op, Op),
	];
	for (outcomes, errors, under_warn, under_fail) in cases {
		let summary = summary_of(outcomes, errors);
		assert_eq!(summary.exit_code(Threshold::Warn), under_warn);
		assert_eq!(summary.exit_code(Threshold::Fail), under_fail);
	}
}

#[test]
fn exit_code_numbers() {
	let codes = [
		(ExitCode::BelowThreshold, 0),
		(ExitCode::Warning, 1),
		(ExitCode::Failure, 2),
		(ExitCode::ConfigurationOrIntegrity, 3),
		(ExitCode::Operational, 4),
	];
	for (exit, code) in codes {
		assert_eq!(exit.code(), code);
		assert_eq!(exit.passes(), code == 0);
	}
	assert_eq!(USAGE_EXIT_CODE, 64);
}

#[test]
fn summary_counts_outcomes_and_errors_apart() {
	let summary = summary_of(
		&[Outcome::Warn, Outcome::Warn, Outcome::Notice],
		&[ErrorKind::Integrity],
	);
	assert_eq!(summary.outcomes(Outcome::Warn), 2);
	assert_eq!(summary.outcomes(Outcome::Notice), 1);
	assert_eq!(summary.outcomes(Outcome::Fail), 0);
	assert_eq!(summary.errors(ErrorKind::Integrity), 1);
	assert_eq!(summary.errors(ErrorKind::Configuration), 0);
}

#[test]
fn a_policy_error_exits_3_under_both_thresholds() {
	let error =
		select_context(Some("prodution"), &example_contexts(), 1).unwrap_err();
	let summary = summary_of(&[], &[error.kind()]);
	for threshold in [Threshold::Warn, Threshold::Fail] {
		assert_eq!(
			summary.exit_code(threshold),
			ExitCode::ConfigurationOrIntegrity
		);
	}
}

#[test]
fn thresholds_parse_exactly_and_default_to_warn() {
	assert_eq!(Threshold::default(), Threshold::Warn);
	for threshold in [Threshold::Warn, Threshold::Fail] {
		assert_eq!(threshold.to_string().parse(), Ok(threshold));
	}
	for invalid in ["notice", "pass", "Warn", " fail", ""] {
		assert!(invalid.parse::<Threshold>().is_err(), "{invalid:?}");
	}
	assert_eq!(
		"error".parse::<Threshold>().unwrap_err().to_string(),
		"`error` is not a threshold; use `warn` or `fail`"
	);
}
