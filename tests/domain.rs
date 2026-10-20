use cutcraft::{
    edit, id,
    project::{self, array, text},
    schema,
    store::Store,
    time::Time,
};
use serde_json::{json, Value};

fn t(n: i64) -> Value {
    json!({"num":n,"den":1})
}
fn range(s: i64, d: i64) -> Value {
    json!({"start":t(s),"duration":t(d)})
}
fn generator(start: i64, duration: i64) -> Value {
    json!({"id":id("item"),"type":"generator","generatorType":"builtin.color","parameters":{"color":{"r":1,"g":0,"b":0,"a":1}},"timelineRange":range(start,duration)})
}
fn track(items: Vec<Value>) -> Value {
    json!({"id":id("track"),"kind":"video","name":"V1","overlapPolicy":"forbid","items":items})
}
fn fixture() -> Value {
    let mut p = project::new("test");
    let sid = text(&p["rootSequenceId"]).to_string();
    p["sequences"][&sid]["tracks"] = json!([track(vec![generator(0, 4), generator(4, 4)])]);
    p
}
fn apply(p: &Value, c: Value) -> anyhow::Result<(Value, Value)> {
    edit::apply(p, &edit::operation(p, c), false)
}
fn sid(p: &Value) -> String {
    text(&p["rootSequenceId"]).into()
}
fn items(p: &Value) -> &[Value] {
    array(&p["sequences"][text(&p["rootSequenceId"])]["tracks"][0]["items"])
}
fn editorial(mut p: Value) -> Value {
    p.as_object_mut().unwrap().remove("revision");
    p.as_object_mut().unwrap().remove("updatedAt");
    p
}

#[test]
fn upstream_contract_fixtures() {
    for input in [
        include_str!("../spec/archive/0.3.0/examples/minimal.project.json"),
        include_str!("../spec/archive/0.3.0/examples/full.project.json"),
    ] {
        project::validate(&serde_json::from_str(input).unwrap()).unwrap();
    }
    schema::validate(
        "operation",
        &serde_json::from_str(include_str!(
            "../spec/archive/0.3.0/examples/batch.operation.json"
        ))
        .unwrap(),
    )
    .unwrap();
    for line in include_str!("../spec/archive/0.3.0/examples/history.jsonl").lines() {
        schema::validate("event", &serde_json::from_str(line).unwrap()).unwrap();
    }
}
#[test]
fn exact_arithmetic_and_checked_serialization() {
    let frame: Time = "1001/30000".parse().unwrap();
    let seconds = frame.mul(&Time::integer(30000));
    assert_eq!(seconds, Time::integer(1001));
    assert_eq!(
        "0.1s".parse::<Time>().unwrap().add(&"0.2".parse().unwrap()),
        "3/10".parse().unwrap()
    );
    assert_eq!("0/100".parse::<Time>().unwrap().json().unwrap(), t(0));
    assert!("1/0".parse::<Time>().is_err());
    assert!("-0.1".parse::<Time>().is_err());
    let huge = Time::integer(i64::MAX);
    assert!(huge.add(&huge).json().is_err());
    assert!(huge.mul(&huge) > huge);
}
#[test]
fn reject_unknown_core_and_preserve_opaque_json() {
    let mut p = fixture();
    p["mystery"] = json!(1);
    assert!(project::validate(&p).is_err());
    p.as_object_mut().unwrap().remove("mystery");
    p["extensions"] = json!({"com.example.plugin":{"num":2,"den":4,"unrecognized":[1,2]}});
    let (new, event) = apply(
        &p,
        json!({"type":"project.update","changes":{"name":"renamed"}}),
    )
    .unwrap();
    assert_eq!(p["extensions"], new["extensions"]);
    schema::validate("event", &event).unwrap();
}
#[test]
fn uuid_and_canonical_time_enforced() {
    let mut p = fixture();
    p["id"] = json!("project_00000000-0000-4000-8000-000000000000");
    assert!(project::validate(&p).is_err());
    let mut p = fixture();
    let s = sid(&p);
    p["sequences"][&s]["tracks"][0]["items"][0]["timelineRange"]["start"] =
        json!({"num":0,"den":5});
    assert!(project::validate(&p).is_err());
    let p = fixture();
    let mut c = json!({"type":"item.move","sequenceId":sid(&p),"itemId":items(&p)[0]["id"],"timelineStart":{"num":2,"den":2},"ripple":"insert"});
    let (p, event) = apply(&p, c.clone()).unwrap();
    assert_eq!(items(&p)[0]["timelineRange"]["start"], t(1));
    assert_eq!(event["command"]["timelineStart"], t(1));
    c["timelineStart"] = json!(1.5);
    assert!(apply(&p, c).is_err());
}
#[test]
fn batch_is_atomic_and_revision_increments_once() {
    let p = fixture();
    let before = p.clone();
    let (p,e)=apply(&p,json!({"type":"batch","commands":[{"type":"project.update","changes":{"name":"a"}},{"type":"project.update","changes":{"name":"b"}}]})).unwrap();
    assert_eq!(p["revision"], 1);
    assert_eq!(e["revisionAfter"], 1);
    assert!(apply(&before,json!({"type":"batch","commands":[{"type":"project.update","changes":{"name":"changed"}},{"type":"asset.remove","assetId":id("asset")}]})).is_err());
    assert_eq!(before["name"], "test");
    let mut stale = edit::operation(
        &p,
        json!({"type":"project.update","changes":{"name":"wrong"}}),
    );
    stale["baseRevision"] = json!(0);
    assert!(edit::apply(&p, &stale, false).is_err());
}
#[test]
fn no_overlap_cycle_or_duplicate_ids() {
    let mut p = fixture();
    let s = sid(&p);
    p["sequences"][&s]["tracks"][0]["items"][1]["timelineRange"]["start"] = t(3);
    assert!(project::validate(&p).is_err());
    let mut p = fixture();
    let s = sid(&p);
    p["sequences"][&s]["tracks"][0]["items"][1]["id"] = items(&p)[0]["id"].clone();
    assert!(project::validate(&p).is_err());
    let mut p = project::new("cycle");
    let s = sid(&p);
    p["sequences"][&s]["tracks"] = json!([track(vec![
        json!({"id":id("item"),"type":"sequence","sequenceId":s,"timelineRange":range(0,1),"sourceRange":range(0,1)})
    ])]);
    assert!(project::validate(&p).is_err());
}
#[test]
fn locked_tracks_cannot_be_bypassed_via_aggregate_or_batch() {
    let mut p = fixture();
    let s = sid(&p);
    p["sequences"][&s]["tracks"][0]["locked"] = json!(true);
    let c = json!({"type":"item.move","sequenceId":s,"itemId":items(&p)[0]["id"],"timelineStart":t(10),"ripple":"none"});
    assert!(apply(&p, c.clone()).is_err());
    assert!(edit::apply(&p, &edit::operation(&p, c), true).is_ok());
    let mut seq = p["sequences"][&s].clone();
    seq["tracks"][0]["locked"] = json!(false);
    assert!(apply(&p,json!({"type":"batch","commands":[{"type":"sequence.put","sequenceId":s,"sequence":seq},{"type":"project.update","changes":{"name":"x"}}]})).is_err());
}
#[test]
fn split_then_inverse_restores_project() {
    let p = fixture();
    let left = id("item");
    let right = id("item");
    let (next,e)=apply(&p,json!({"type":"item.split","sequenceId":sid(&p),"itemId":items(&p)[0]["id"],"at":t(1),"leftItemId":left,"rightItemId":right})).unwrap();
    assert_eq!(items(&next).len(), 3);
    assert_eq!(items(&next)[0]["timelineRange"], range(0, 1));
    let (restored, _) = apply(
        &next,
        json!({"type":"batch","commands":e["inverseCommands"]}),
    )
    .unwrap();
    assert_eq!(editorial(p), editorial(restored));
}
#[test]
fn split_fit_reverse_preserves_source_mapping() {
    let mut p = fixture();
    let s = sid(&p);
    let aid = id("asset");
    p["assets"][&aid] = json!({"name":"video","kind":"media","storage":"external","availability":"offline","uri":"x.mp4","streams":[{"id":"stream_v0","index":0,"kind":"video","duration":t(100),"video":{"width":100,"height":100,"pixelAspect":t(1),"frameRate":t(30)}}]});
    let iid = items(&p)[0]["id"].clone();
    p["sequences"][&s]["tracks"][0]["items"][0] = json!({"id":iid,"type":"media","assetId":aid,"streamId":"stream_v0","timelineRange":range(0,4),"sourceRange":range(10,8),"playback":{"mode":"fit","reverse":true}});
    let (next,_)=apply(&p,json!({"type":"item.split","sequenceId":s,"itemId":iid,"at":t(1),"leftItemId":id("item"),"rightItemId":id("item")})).unwrap();
    assert_eq!(items(&next)[0]["sourceRange"], range(16, 2));
    assert_eq!(items(&next)[1]["sourceRange"], range(10, 6));
}
#[test]
fn link_movement_and_split_are_group_edits() {
    let mut p = fixture();
    let s = sid(&p);
    let second = track(vec![generator(0, 4)]);
    let b = second["items"][0]["id"].clone();
    let a = items(&p)[0]["id"].clone();
    p["sequences"][&s]["tracks"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let (p,_)=apply(&p,json!({"type":"link.put","sequenceId":s,"link":{"id":id("link"),"kind":"sync","itemIds":[a,b]}})).unwrap();
    let (p, _) = apply(
        &p,
        json!({"type":"item.move","sequenceId":s,"itemId":a,"timelineStart":t(8),"ripple":"none"}),
    )
    .unwrap();
    assert_eq!(
        p["sequences"][&s]["tracks"][1]["items"][0]["timelineRange"]["start"],
        t(8)
    );
    let (p,_)=apply(&p,json!({"type":"item.split","sequenceId":s,"itemId":a,"at":t(10),"leftItemId":id("item"),"rightItemId":id("item")})).unwrap();
    assert_eq!(array(&p["sequences"][&s]["links"]).len(), 2);
    project::validate(&p).unwrap();
}
#[test]
fn ripple_overwrite_can_split_a_target_and_is_undoable() {
    let mut p = fixture();
    let s = sid(&p);
    let new = generator(10, 2);
    let iid = new["id"].clone();
    p["sequences"][&s]["tracks"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(new);
    let (next,e)=apply(&p,json!({"type":"item.move","sequenceId":s,"itemId":iid,"timelineStart":t(1),"ripple":"overwrite"})).unwrap();
    assert_eq!(items(&next).len(), 4);
    assert_eq!(items(&next)[0]["timelineRange"], range(0, 1));
    assert_eq!(items(&next)[2]["timelineRange"], range(3, 1));
    let (restored, _) = apply(
        &next,
        json!({"type":"batch","commands":e["inverseCommands"]}),
    )
    .unwrap();
    assert_eq!(editorial(p), editorial(restored));
}
#[test]
fn references_require_cascade_and_link_backrefs_are_checked() {
    let p = fixture();
    let a = items(&p)[0]["id"].clone();
    let b = items(&p)[1]["id"].clone();
    let s = sid(&p);
    let (p,_)=apply(&p,json!({"type":"link.put","sequenceId":s,"link":{"id":id("link"),"kind":"group","itemIds":[a,b]}})).unwrap();
    assert!(apply(&p, json!({"type":"item.remove","sequenceId":s,"itemId":a})).is_err());
    let (p, _) = apply(
        &p,
        json!({"type":"item.remove","sequenceId":s,"itemId":a,"cascade":true}),
    )
    .unwrap();
    assert!(items(&p).is_empty());
    assert!(array(&p["sequences"][&s]["links"]).is_empty());
}
#[test]
fn sample_half_open_and_solo_rules() {
    let mut p = fixture();
    let s = sid(&p);
    let snapshot = project::sample(&p, &s, &Time::integer(4)).unwrap();
    assert_eq!(snapshot["layers"][0]["itemId"], items(&p)[1]["id"]);
    assert_eq!(array(&snapshot["layers"]).len(), 1);
    let mut second = track(vec![generator(0, 8)]);
    second["solo"] = json!(true);
    let tid = second["id"].clone();
    p["sequences"][&s]["tracks"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let snapshot = project::sample(&p, &s, &Time::integer(1)).unwrap();
    assert_eq!(array(&snapshot["layers"]).len(), 1);
    assert_eq!(snapshot["layers"][0]["trackId"], tid);
}
#[test]
fn durable_apply_dry_run_rejection_undo_and_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path(), "before").unwrap();
    let op = edit::operation(
        &store.project().unwrap(),
        json!({"type":"project.update","changes":{"name":"after"}}),
    );
    store.apply(&op, false, true).unwrap();
    assert_eq!(store.project().unwrap()["name"], "before");
    assert!(store.history().unwrap().is_empty());
    store.apply(&op, false, false).unwrap();
    assert_eq!(store.project().unwrap()["revision"], 1);
    assert!(store.apply(&op, false, false).is_err());
    let mut stale = edit::operation(
        &store.project().unwrap(),
        json!({"type":"project.update","changes":{"name":"bad"}}),
    );
    stale["baseRevision"] = json!(0);
    assert!(store.apply(&stale, false, false).is_err());
    assert_eq!(
        store.history().unwrap().last().unwrap()["status"],
        "rejected"
    );
    assert_eq!(store.project().unwrap()["revision"], 1);
    store.undo(false, false).unwrap();
    let history = store.history().unwrap();
    assert_eq!(
        history.last().unwrap()["metadata"]["undoOf"],
        history[0]["eventId"]
    );
    assert_eq!(store.project().unwrap()["name"], "before");
    assert_eq!(store.project().unwrap()["revision"], 2);
    let p = store.project().unwrap();
    let history = store.history().unwrap();
    let log = history
        .iter()
        .map(|e| format!("{}\n", serde_json::to_string(e).unwrap()))
        .collect::<String>();
    std::fs::write(
        temp.path().join(".cutcraft/transaction.json"),
        serde_json::to_vec(&json!({"project":p,"history":log})).unwrap(),
    )
    .unwrap();
    std::fs::write(temp.path().join("cutcraft.json"), "interrupted write").unwrap();
    drop(store);
    let recovered = Store::open(temp.path()).unwrap();
    assert_eq!(recovered.project().unwrap(), p);
    assert_eq!(recovered.history().unwrap(), history);
    assert!(!temp.path().join(".cutcraft/transaction.json").exists());
}

#[test]
fn remaining_command_variants_and_aggregate_inverses() {
    let p = fixture();
    let s = sid(&p);
    let tid = p["sequences"][&s]["tracks"][0]["id"].clone();
    let aid = id("asset");
    let seqid = id("seq");
    let marker = id("marker");
    let transition = id("transition");
    let commands = vec![
        json!({"type":"asset.put","assetId":aid,"asset":{"name":"font","kind":"font","uri":"font.ttf","storage":"external","availability":"offline","streams":[]}}),
        json!({"type":"sequence.put","sequenceId":seqid,"sequence":project::new("Nested")["sequences"].as_object().unwrap().values().next().unwrap()}),
        json!({"type":"marker.put","sequenceId":s,"marker":{"id":marker,"name":"cut","start":t(4),"duration":t(0)}}),
        json!({"type":"transition.put","sequenceId":s,"transition":{"id":transition,"fromItemId":items(&p)[0]["id"],"toItemId":items(&p)[1]["id"],"transitionType":"builtin.crossfade","version":"2.0.0","duration":t(1),"alignment":"center"}}),
        json!({"type":"track.reorder","sequenceId":s,"trackIds":[tid]}),
    ];
    let (next, event) = apply(&p, json!({"type":"batch","commands":commands})).unwrap();
    let (restored, _) = apply(
        &next,
        json!({"type":"batch","commands":event["inverseCommands"]}),
    )
    .unwrap();
    assert_eq!(editorial(p.clone()), editorial(restored));
    let (clean, _) = apply(
        &next,
        json!({"type":"batch","commands":[
            {"type":"marker.remove","sequenceId":s,"markerId":marker},
            {"type":"transition.remove","sequenceId":s,"transitionId":transition},
            {"type":"asset.remove","assetId":aid},
            {"type":"sequence.remove","sequenceId":seqid},
            {"type":"track.remove","sequenceId":s,"trackId":tid}
        ]}),
    )
    .unwrap();
    assert!(array(&clean["sequences"][&s]["tracks"]).is_empty());
}
#[test]
fn remove_gap_merges_multiple_intervals_and_does_not_double_shift_links() {
    let mut p = fixture();
    let s = sid(&p);
    let a = items(&p)[0]["id"].clone();
    let b = items(&p)[1]["id"].clone();
    p["sequences"][&s]["tracks"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(generator(8, 2));
    let (p,_)=apply(&p,json!({"type":"link.put","sequenceId":s,"link":{"id":id("link"),"kind":"group","itemIds":[a,b]}})).unwrap();
    let (p,_)=apply(&p,json!({"type":"item.remove","sequenceId":s,"itemId":a,"ripple":"remove-gap","cascade":true})).unwrap();
    assert_eq!(items(&p)[0]["timelineRange"], range(0, 2));
    let mut p = fixture();
    let s = sid(&p);
    p["sequences"][&s]["tracks"][0]["items"][1]["timelineRange"]["start"] = t(8);
    let second = track(vec![generator(0, 4), generator(8, 4)]);
    let a = items(&p)[0]["id"].clone();
    let b = items(&p)[1]["id"].clone();
    let c = second["items"][0]["id"].clone();
    let d = second["items"][1]["id"].clone();
    p["sequences"][&s]["tracks"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let (p,_)=apply(&p,json!({"type":"batch","commands":[
        {"type":"link.put","sequenceId":s,"link":{"id":id("link"),"kind":"sync","itemIds":[a,c]}},
        {"type":"link.put","sequenceId":s,"link":{"id":id("link"),"kind":"sync","itemIds":[b,d]}}
    ]})).unwrap();
    let (p,_)=apply(&p,json!({"type":"item.remove","sequenceId":s,"itemId":a,"ripple":"remove-gap","cascade":true})).unwrap();
    assert_eq!(items(&p)[0]["timelineRange"]["start"], t(4));
    assert_eq!(
        p["sequences"][&s]["tracks"][1]["items"][0]["timelineRange"]["start"],
        t(4)
    );
}
#[test]
fn reverse_nested_sampling_uses_exact_exclusive_upper_boundary() {
    let mut p = fixture();
    let s = sid(&p);
    let outer = id("seq");
    let mut seq = project::new("outer")["sequences"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    seq["tracks"] = json!([track(vec![
        json!({"id":id("item"),"type":"sequence","sequenceId":s,"timelineRange":range(0,8),"sourceRange":range(0,8),"playback":{"reverse":true}})
    ])]);
    p["sequences"][&outer] = seq;
    p["rootSequenceId"] = json!(outer);
    let sampled = project::sample(&p, &outer, &Time::zero()).unwrap();
    assert_eq!(sampled["layers"][0]["sourceTime"], t(8));
    assert_eq!(
        sampled["layers"][0]["children"][0]["itemId"],
        p["sequences"][&s]["tracks"][0]["items"][1]["id"]
    );
    let at_cut = project::sample(&p, &outer, &Time::integer(4)).unwrap();
    assert_eq!(
        at_cut["layers"][0]["children"][0]["itemId"],
        p["sequences"][&s]["tracks"][0]["items"][0]["id"]
    );
}
#[test]
fn malformed_core_inputs_never_panic() {
    let inputs = [
        Value::Null,
        json!([]),
        json!({}),
        json!({"specVersion":"0.3.0"}),
        json!({"num":1,"den":0}),
    ];
    for input in inputs {
        assert!(project::validate(&input).is_err());
        assert!(schema::validate("operation", &input).is_err());
    }
}

#[test]
fn builtins_publish_and_enforce_parameter_contracts() {
    schema::validate_generator(&generator(0, 1)).unwrap();
    let mut item = generator(0, 1);
    item["parameters"]["unknown"] = json!(true);
    assert!(schema::validate_generator(&item).is_err());
    item["generatorType"] = json!("com.example.generator");
    assert!(schema::validate_generator(&item).is_err());
    // Custom data remains portable at the project layer.
    let mut p = fixture();
    let s = sid(&p);
    p["sequences"][&s]["tracks"][0]["items"][0] = item;
    project::validate(&p).unwrap();
}
#[test]
fn concurrent_cli_writers_recheck_revision_under_process_lock() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path(), "concurrent").unwrap();
    drop(store);
    let command = temp.path().join("rename.json");
    std::fs::write(
        &command,
        r#"{"type":"project.update","changes":{"name":"winner"}}"#,
    )
    .unwrap();
    let spawn = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_cutcraft"))
            .arg("--project")
            .arg(temp.path())
            .arg("command")
            .arg(&command)
            .args(["--base-revision", "0"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut first = spawn();
    let mut second = spawn();
    let statuses = [
        first.wait().unwrap().success(),
        second.wait().unwrap().success(),
    ];
    assert_eq!(statuses.iter().filter(|ok| **ok).count(), 1);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(store.project().unwrap()["revision"], 1);
    let history = store.history().unwrap();
    // A contender either fails before acquiring the lock (no event), or
    // acquires it after the winner and records a revision conflict.
    assert!((1..=2).contains(&history.len()));
    assert_eq!(
        history.iter().filter(|e| e["status"] == "applied").count(),
        1
    );
}

#[test]
fn application_versions_are_hashed_and_survive_undo() {
    let mut p = project::new("client history");
    let old = json!({"id":"cutcraft.desktop","kind":"desktop","version":"0.0.1"});
    p["createdWith"] = old.clone();
    p["lastModifiedWith"] = old.clone();
    p["editors"] = json!([old]);
    let (next, event) = apply(
        &p,
        json!({"type":"project.update","changes":{"name":"changed"}}),
    )
    .unwrap();
    assert_eq!(next["createdWith"], p["createdWith"]);
    assert_eq!(next["lastModifiedWith"], project::application());
    assert_eq!(next["editors"].as_array().unwrap().len(), 2);
    assert_eq!(
        event["projectHashAfter"],
        cutcraft::store::hash(&next).unwrap()
    );
    let (undo, _) = apply(
        &next,
        json!({"type":"batch","commands":event["inverseCommands"]}),
    )
    .unwrap();
    assert_eq!(undo["name"], p["name"]);
    assert_eq!(undo["editors"], next["editors"]);
}

#[test]
fn lock_contention_is_nonblocking_and_legacy_is_protected() {
    let tmp = tempfile::tempdir().unwrap();
    let held = Store::init(tmp.path(), "locked").unwrap();
    assert!(tmp.path().join("cutcraft.json").is_file());
    assert!(!tmp.path().join("project.json").exists());
    let lock_before = std::fs::read(tmp.path().join(".cutcraft.lock")).unwrap();
    let metadata: Value = serde_json::from_slice(&lock_before).unwrap();
    assert_eq!(metadata["pid"], std::process::id());
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .arg("-p")
        .arg(tmp.path())
        .arg("validate")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("PROJECT_LOCKED"));
    assert_eq!(
        lock_before,
        std::fs::read(tmp.path().join(".cutcraft.lock")).unwrap()
    );
    drop(held);
    Store::open(tmp.path()).unwrap();
    let legacy = tempfile::tempdir().unwrap();
    std::fs::write(legacy.path().join("project.json"), b"legacy").unwrap();
    assert!(Store::open(legacy.path()).is_err());
    assert!(Store::init(legacy.path(), "do not overwrite").is_err());
    assert_eq!(
        std::fs::read(legacy.path().join("project.json")).unwrap(),
        b"legacy"
    );
    assert!(!legacy.path().join("cutcraft.json").exists());
}

#[test]
fn lock_holder_child() {
    let Ok(dir) = std::env::var("CUTCRAFT_TEST_LOCK_DIR") else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let _store = Store::open(dir).unwrap();
    std::fs::write(dir.join("ready"), b"ready").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}

#[test]
fn killed_lock_holder_does_not_leave_project_locked() {
    let tmp = tempfile::tempdir().unwrap();
    drop(Store::init(tmp.path(), "crash").unwrap());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_holder_child"])
        .env("CUTCRAFT_TEST_LOCK_DIR", tmp.path())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !tmp.path().join("ready").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let ready = tmp.path().join("ready").exists();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(ready, "child did not acquire lock");
    assert!(tmp.path().join(".cutcraft.lock").exists());
    Store::open(tmp.path()).unwrap();
}

#[test]
fn color_effect_parameters_curves_and_lut_references_are_validated() {
    let mut p = fixture();
    let s = sid(&p);
    let effect = json!({"id":id("effect"),"effectType":"builtin.color-grade","version":"1.0.0","parameters":{"exposureEv":1,"contrast":1.1}});
    p["sequences"][&s]["tracks"][0]["items"][0]["effects"] = json!([effect]);
    project::validate(&p).unwrap();
    fn effects(p: &mut Value) -> &mut Value {
        let s = sid(p);
        &mut p["sequences"][&s]["tracks"][0]["items"][0]["effects"][0]
    }
    effects(&mut p)["parameters"]["contrast"] = json!(-1);
    assert!(project::validate(&p).is_err());
    effects(&mut p)["effectType"] = json!("builtin.curves");
    effects(&mut p)["parameters"] = json!({"rgb":[{"x":0,"y":0.1},{"x":1,"y":0.9}]});
    project::validate(&p).unwrap();
    effects(&mut p)["parameters"]["rgb"] = json!([{"x":0,"y":0},{"x":0,"y":0.5},{"x":1,"y":1}]);
    assert!(project::validate(&p).is_err());
    let aid = id("asset");
    effects(&mut p)["effectType"] = json!("builtin.lut");
    effects(&mut p)["parameters"] = json!({"assetId":aid,"strength":1,"inputColorSpace":"rec709","outputColorSpace":"rec709","interpolation":"tetrahedral"});
    assert!(project::validate(&p).is_err());
    p["assets"][&aid] = json!({"name":"look.cube","kind":"lut","uri":"assets/look.cube","storage":"managed","availability":"online","streams":[]});
    project::validate(&p).unwrap();
    assert!(apply(&p, json!({"type":"asset.remove","assetId":aid})).is_err());
    let (removed, _) = apply(
        &p,
        json!({"type":"asset.remove","assetId":aid,"cascade":true}),
    )
    .unwrap();
    assert_eq!(items(&removed).len(), 1);
}
