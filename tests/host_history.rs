use cutcraft::{edit, project, store::Store};
use serde_json::{json, Value};

fn rename(store: &Store, name: &str) -> Value {
    store
        .command(
            json!({"type":"project.update","changes":{"name":name}}),
            false,
            false,
        )
        .unwrap()
}

#[test]
fn shared_history_rebuilds_multistep_undo_redo_and_keeps_legacy_undo() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path(), "A").unwrap();
    rename(&store, "B");
    rename(&store, "C");
    store.history_undo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "B");
    store.history_undo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "A");
    assert_eq!(store.history_state().unwrap()["canUndo"], false);
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    store.history_redo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "B");
    store.history_redo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "C");
    assert_eq!(store.project().unwrap()["revision"], 6);
    store.undo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "B");
    assert_eq!(store.history_state().unwrap()["canRedo"], false);
    store.history_undo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "C");
    rename(&store, "D");
    assert_eq!(store.history_state().unwrap()["canRedo"], false);
}

#[test]
fn history_dry_run_rejection_and_untrusted_markers_do_not_corrupt_stacks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path(), "A").unwrap();
    let first = rename(&store, "B");
    let snapshot = store.project().unwrap();
    let history = store.history().unwrap();
    store.history_undo(false, true).unwrap();
    assert_eq!(store.project().unwrap(), snapshot);
    assert_eq!(store.history().unwrap(), history);
    let mut forged = edit::operation(
        &snapshot,
        json!({"type":"project.update","changes":{"name":"C"}}),
    );
    forged["metadata"] = json!({"cutcraft.history":{"version":1,"action":"undo","targetEventId":first["event"]["eventId"]}});
    store.apply(&forged, false, false).unwrap();
    assert_eq!(store.history_state().unwrap()["canRedo"], false);
    store.history_undo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "B");
    let mut stale = edit::operation(
        &snapshot,
        json!({"type":"project.update","changes":{"name":"stale"}}),
    );
    stale["baseRevision"] = json!(0);
    assert!(store.apply(&stale, false, false).is_err());
    assert_eq!(store.history_state().unwrap()["canRedo"], true);
    store.history_redo(false, false).unwrap();
    assert_eq!(store.project().unwrap()["name"], "C");
}

#[test]
fn attribution_is_scoped_hashed_and_survives_shared_undo() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path(), "A").unwrap();
    let web = json!({"id":"cutcraft.web","kind":"web","version":env!("CARGO_PKG_VERSION")});
    let actor = json!({"type":"user","id":cutcraft::id("actor"),"displayName":"Browser"});
    let result = project::with_context(web.clone(), actor.clone(), || rename(&store, "B"));
    assert_eq!(result["event"]["actor"], actor);
    assert_eq!(result["project"]["lastModifiedWith"], web);
    assert_eq!(
        result["event"]["projectHashAfter"],
        cutcraft::store::hash(&result["project"]).unwrap()
    );
    assert_eq!(project::application()["id"], "cutcraft.cli");
    store.history_undo(false, false).unwrap();
    assert!(store.project().unwrap()["editors"]
        .as_array()
        .unwrap()
        .contains(&web));
}

#[test]
fn shared_history_preserves_every_supported_project_version() {
    for fixture in [
        "spec/archive/0.3.0/examples/minimal.project.json",
        "spec/archive/0.4.0/examples/animated.cutcraft.json",
        "spec/archive/0.5.0/examples/image-sequence.cutcraft.json",
    ] {
        let original: Value = serde_json::from_slice(
            &std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture)).unwrap(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cutcraft.json"),
            project::canonical(&original).unwrap(),
        )
        .unwrap();
        let store = Store::open(dir.path()).unwrap();
        rename(&store, "first");
        rename(&store, "second");
        let first = store.history().unwrap()[0].clone();
        store.history_undo(false, false).unwrap();
        store.history_undo(false, false).unwrap();
        let restored = store.project().unwrap();
        for key in [
            "specVersion",
            "$schema",
            "id",
            "name",
            "assets",
            "sequences",
        ] {
            assert_eq!(restored[key], original[key], "{fixture}: {key}");
        }
        store.history_redo(false, false).unwrap();
        store.history_redo(false, false).unwrap();
        assert_eq!(store.project().unwrap()["name"], "second");
        assert_eq!(store.history().unwrap()[0], first);
        assert!(store
            .history()
            .unwrap()
            .iter()
            .all(|event| event["eventVersion"] == original["specVersion"]));
    }
}

#[test]
fn decimal_color_roundtrip_keeps_committed_history_hash() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path(), "Color").unwrap();
    let project = store.project().unwrap();
    let result=store.command(json!({"type":"track.put","sequenceId":project["rootSequenceId"],"track":{"id":cutcraft::id("track"),"kind":"video","name":"Color","overlapPolicy":"forbid","items":[{"id":cutcraft::id("item"),"type":"generator","generatorType":"builtin.color","version":"1.0.0","parameters":{"color":{"r":40.0/255.0,"g":100.0/255.0,"b":200.0/255.0,"a":1.0}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}}]}}),false,false).unwrap();
    assert_eq!(
        cutcraft::store::hash(&store.project().unwrap()).unwrap(),
        result["event"]["projectHashAfter"]
    );
    store.history_undo(false, false).unwrap();
    store.history_redo(false, false).unwrap();
    assert_eq!(
        store.project().unwrap()["sequences"],
        result["project"]["sequences"]
    );
}
