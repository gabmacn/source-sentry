//! Tests of the adapter kinds.

use super::*;
use crate::testing::path;

#[test]
fn adapters_use_the_config_spelling() {
	for (adapter, name) in
		[(Adapter::MdUtf8, "md-utf8"), (Adapter::Opaque, "opaque")]
	{
		assert_eq!(adapter.name(), name);
		assert_eq!(adapter.to_string(), name);
		assert_eq!(
			serde_json::to_string(&adapter).unwrap(),
			format!("\"{name}\"")
		);
		assert_eq!(
			serde_json::from_str::<Adapter>(&format!("\"{name}\"")).unwrap(),
			adapter
		);
	}
}

#[test]
fn only_md_utf8_documents_carry_a_declaration() {
	let bytes = b"---\nsource-sentry: {owners: ['legal']}\n---\n";
	let document = path("docs/a.pdf");
	assert!(Adapter::MdUtf8.carries_declaration());
	assert!(!Adapter::Opaque.carries_declaration());
	assert!(
		Adapter::MdUtf8
			.read_declaration(&document, bytes)
			.unwrap()
			.is_some()
	);
	assert_eq!(Adapter::Opaque.read_declaration(&document, bytes), Ok(None));
}
