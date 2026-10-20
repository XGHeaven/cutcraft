//! Offline Draft 2020-12 validation against the Cutcraft spec.
use anyhow::{bail, Result};
use jsonschema::{Resource, Validator};
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

pub const SCHEMAS: [(&str, &str); 7] = [
    (
        "animation",
        include_str!("../spec/schemas/animation.schema.json"),
    ),
    ("effect", include_str!("../spec/schemas/effect.schema.json")),
    (
        "generator",
        include_str!("../schemas/generator.schema.json"),
    ),
    ("common", include_str!("../spec/schemas/common.schema.json")),
    (
        "project",
        include_str!("../spec/schemas/project.schema.json"),
    ),
    (
        "operation",
        include_str!("../spec/schemas/operation.schema.json"),
    ),
    ("event", include_str!("../spec/schemas/event.schema.json")),
];
fn validators() -> &'static BTreeMap<&'static str, Validator> {
    static VALIDATORS: OnceLock<BTreeMap<&'static str, Validator>> = OnceLock::new();
    VALIDATORS.get_or_init(|| {
        let internal_05 = [
            (
                "internal05-common",
                include_str!("../spec/archive/0.5.0/schemas/common.schema.json"),
            ),
            (
                "internal05-animation",
                include_str!("../spec/archive/0.5.0/schemas/animation.schema.json"),
            ),
            (
                "internal05-effect",
                include_str!("../spec/archive/0.5.0/schemas/effect.schema.json"),
            ),
            (
                "internal05-project",
                include_str!("../spec/archive/0.5.0/schemas/project.schema.json"),
            ),
            (
                "internal05-operation",
                include_str!("../spec/archive/0.5.0/schemas/operation.schema.json"),
            ),
            (
                "internal05-event",
                include_str!("../spec/archive/0.5.0/schemas/event.schema.json"),
            ),
        ];
        let previous = [
            (
                "previous-common",
                include_str!("../spec/archive/0.4.0/schemas/common.schema.json"),
            ),
            (
                "previous-animation",
                include_str!("../spec/archive/0.4.0/schemas/animation.schema.json"),
            ),
            (
                "previous-effect",
                include_str!("../spec/archive/0.4.0/schemas/effect.schema.json"),
            ),
            (
                "previous-project",
                include_str!("../spec/archive/0.4.0/schemas/project.schema.json"),
            ),
            (
                "previous-operation",
                include_str!("../spec/archive/0.4.0/schemas/operation.schema.json"),
            ),
            (
                "previous-event",
                include_str!("../spec/archive/0.4.0/schemas/event.schema.json"),
            ),
        ];
        let legacy = [
            (
                "legacy-common",
                include_str!("../spec/archive/0.3.0/schemas/common.schema.json"),
            ),
            (
                "legacy-project",
                include_str!("../spec/archive/0.3.0/schemas/project.schema.json"),
            ),
            (
                "legacy-operation",
                include_str!("../spec/archive/0.3.0/schemas/operation.schema.json"),
            ),
            (
                "legacy-event",
                include_str!("../spec/archive/0.3.0/schemas/event.schema.json"),
            ),
            (
                "legacy-effect",
                include_str!("../spec/archive/0.3.0/schemas/effect.schema.json"),
            ),
        ];
        let values: Vec<_> = SCHEMAS
            .iter()
            .chain(legacy.iter())
            .chain(previous.iter())
            .chain(internal_05.iter())
            .map(|(name, text)| {
                (
                    *name,
                    serde_json::from_str::<Value>(text).expect("embedded schema"),
                )
            })
            .collect();
        values
            .iter()
            .map(|(name, value)| {
                let mut options = jsonschema::options().should_validate_formats(true);
                for (_, resource) in &values {
                    options = options.with_resource(
                        resource["$id"].as_str().unwrap(),
                        Resource::from_contents(resource.clone()).expect("embedded resource"),
                    );
                }
                (
                    *name,
                    options.build(value).expect("embedded schema compilation"),
                )
            })
            .collect()
    })
}
pub fn validate(kind: &str, value: &Value) -> Result<()> {
    let legacy = format!("legacy-{kind}");
    let previous = format!("previous-{kind}");
    let internal_05 = format!("internal05-{kind}");
    let kind = if value["specVersion"] == "0.3.0" || value["eventVersion"] == "0.3.0" {
        legacy.as_str()
    } else if value["specVersion"] == "0.4.0" || value["eventVersion"] == "0.4.0" {
        previous.as_str()
    } else if value["specVersion"] == "0.5.0" || value["eventVersion"] == "0.5.0" {
        internal_05.as_str()
    } else {
        kind
    };
    let validator = validators()
        .get(kind)
        .ok_or_else(|| anyhow::anyhow!("unknown schema: {kind}"))?;
    let errors: Vec<_> = validator
        .iter_errors(value)
        .take(20)
        .map(|e| format!("{}: {e}", e.instance_path))
        .collect();
    if !errors.is_empty() {
        bail!("SCHEMA_INVALID ({kind}): {}", errors.join("; "));
    }
    Ok(())
}

/// Parameter validation is separate from the portable project schema.
pub fn validate_generator(item: &Value) -> Result<()> {
    let mut envelope =
        serde_json::json!({"generatorType":item["generatorType"],"parameters":item["parameters"]});
    if let Some(version) = item.get("version") {
        envelope["version"] = version.clone();
    }
    validate("generator", &envelope)
}
