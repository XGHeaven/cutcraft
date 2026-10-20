use cutcraft::{edit, project, render, schema, store, store::Store, SPEC_VERSION};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn load(path: &str) -> Value {
    store::read_json(&Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

#[test]
fn public_examples_and_history_are_a_replayable_contract() {
    for name in [
        "minimal",
        "animated",
        "transition",
        "retime",
        "image-sequence",
        "after-operation",
    ] {
        let p = load(&format!("spec/examples/{name}.cutcraft.json"));
        assert_eq!(p["specVersion"], SPEC_VERSION);
        project::validate(&p).unwrap();
    }
    let before = load("spec/examples/minimal.cutcraft.json");
    let after = load("spec/examples/after-operation.cutcraft.json");
    let op = load("spec/examples/batch.operation.json");
    let event: Value =
        serde_json::from_str(include_str!("../spec/examples/history.jsonl")).unwrap();
    schema::validate("event", &event).unwrap();
    assert_eq!(event["projectHashBefore"], store::hash(&before).unwrap());
    assert_eq!(event["projectHashAfter"], store::hash(&after).unwrap());
    assert_eq!(event["command"], op["command"]);
    let (mut replayed, replay_event) = edit::apply(&before, &op, false).unwrap();
    // Wall-clock edit time varies; all domain data and inverse commands must agree.
    replayed["updatedAt"] = after["updatedAt"].clone();
    assert_eq!(replayed, after);
    assert_eq!(replay_event["inverseCommands"], event["inverseCommands"]);
}

#[test]
fn internal_projects_keep_their_versions_ids_and_history_through_edit_and_undo() {
    for path in [
        "spec/archive/0.3.0/examples/minimal.project.json",
        "spec/archive/0.4.0/examples/animated.cutcraft.json",
        "spec/archive/0.5.0/examples/image-sequence.cutcraft.json",
    ] {
        let original = load(path);
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("cutcraft.json");
        let bytes = project::canonical(&original).unwrap();
        fs::write(&file, &bytes).unwrap();
        let store = Store::open(temp.path()).unwrap();
        assert_eq!(store.project().unwrap(), original);
        assert_eq!(fs::read(&file).unwrap(), bytes);
        store
            .command(
                json!({"type":"project.update","changes":{"name":"Edited by public CLI"}}),
                false,
                false,
            )
            .unwrap();
        let changed = store.project().unwrap();
        let first = store.history().unwrap()[0].clone();
        assert_eq!(first["eventVersion"], original["specVersion"]);
        assert_eq!(first["projectHashBefore"], store::hash(&original).unwrap());
        assert_eq!(first["projectHashAfter"], store::hash(&changed).unwrap());
        store.undo(false, false).unwrap();
        let restored = store.project().unwrap();
        for field in [
            "specVersion",
            "$schema",
            "id",
            "name",
            "assets",
            "sequences",
            "createdWith",
        ] {
            assert_eq!(restored[field], original[field], "{path}: {field}");
        }
        assert_eq!(
            restored["revision"],
            original["revision"].as_u64().unwrap() + 2
        );
        assert_eq!(restored["lastModifiedWith"], project::application());
        let history = store.history().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0], first);
        assert_eq!(history[1]["eventVersion"], original["specVersion"]);
    }
}

#[test]
fn unknown_versions_and_cross_version_operations_are_rejected() {
    let p = project::new("Versions");
    for version in ["0.0.0", "0.2.0", "0.6.0", "1.0.0"] {
        let mut unknown = p.clone();
        unknown["specVersion"] = json!(version);
        assert!(project::validate(&unknown).is_err(), "{version}");
        let mut op = edit::operation(&p, json!({"type":"project.update","changes":{"name":"x"}}));
        op["specVersion"] = json!(version);
        assert!(schema::validate("operation", &op).is_err());
    }
    for version in ["0.3.0", "0.4.0", "0.5.0"] {
        let mut op = edit::operation(&p, json!({"type":"project.update","changes":{"name":"x"}}));
        op["specVersion"] = json!(version);
        schema::validate("operation", &op).unwrap();
        assert!(edit::apply(&p, &op, false)
            .unwrap_err()
            .to_string()
            .contains("SPEC_VERSION_MISMATCH"));
    }
}

#[test]
fn cli_initializes_public_version_and_publishes_current_schemas() {
    let cli = env!("CARGO_BIN_EXE_cutcraft");
    let temp = tempfile::tempdir().unwrap();
    let result = Command::new(cli)
        .arg("init")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    let p = store::read_json(&temp.path().join("cutcraft.json")).unwrap();
    assert_eq!(p["specVersion"], SPEC_VERSION);
    assert_eq!(p["createdWith"]["version"], env!("CARGO_PKG_VERSION"));
    let schema_output = Command::new(cli)
        .args(["schema", "project"])
        .output()
        .unwrap();
    assert!(schema_output.status.success());
    let schema: Value = serde_json::from_slice(&schema_output.stdout).unwrap();
    assert_eq!(schema["properties"]["specVersion"]["const"], SPEC_VERSION);
    assert_eq!(p["$schema"], schema["$id"]);
    let caps = render::capabilities();
    assert!(caps["specVersions"]
        .as_array()
        .unwrap()
        .contains(&json!(SPEC_VERSION)));
    assert_eq!(caps["imageSequences"]["specVersion"], SPEC_VERSION);
}
