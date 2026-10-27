use clap::Parser;
use cutcraft::{
    app::{self, Cli},
    store::Store,
    time::Time,
};
use serde_json::json;

#[test]
fn prepared_json_input_survives_client_file_changes_and_transport() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    drop(Store::init(&dir, "before").unwrap());
    let input = tmp.path().join("command.json");
    std::fs::write(
        &input,
        r#"{"type":"project.update","changes":{"name":"captured"}}"#,
    )
    .unwrap();
    let cli = Cli::try_parse_from([
        "cutcraft",
        "-p",
        dir.to_str().unwrap(),
        "command",
        input.to_str().unwrap(),
    ])
    .unwrap();
    let prepared = app::prepare(cli).unwrap();
    std::fs::write(&input, "not JSON anymore").unwrap();
    let received = serde_json::from_slice(&serde_json::to_vec(&prepared).unwrap()).unwrap();
    let result = app::ApplicationHost.execute(received).unwrap();
    assert_eq!(result["project"]["name"], "captured");
    assert_eq!(result["project"]["revision"], 1);
}

#[test]
fn command_transport_preserves_full_i64_time_precision() {
    let t = Time::new(9_007_199_254_740_993, 7).unwrap();
    let value = serde_json::to_value(&t).unwrap();
    assert_eq!(value, json!({"num":9_007_199_254_740_993_i64,"den":7}));
    assert_eq!(serde_json::from_value::<Time>(value).unwrap(), t);
    assert!(serde_json::from_value::<Time>(json!({"num":1,"den":0})).is_err());
}

#[test]
fn open_close_and_history_are_real_cli_commands() {
    for arguments in [
        vec!["cutcraft", "open", "--no-browser"],
        vec!["cutcraft", "open", "--host", "localhost"],
        vec!["cutcraft", "open", "--host", "0.0.0.0"],
        vec!["cutcraft", "open", "--host", "::1"],
        vec!["cutcraft", "-p", "some-project", "close"],
        vec!["cutcraft", "history"],
        vec!["cutcraft", "history", "undo"],
        vec!["cutcraft", "history", "redo"],
    ] {
        assert!(Cli::try_parse_from(arguments).is_ok());
    }
}

#[test]
fn headless_host_keeps_dry_run_and_editor_metadata_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    drop(Store::init(&dir, "before").unwrap());
    let before = std::fs::read(dir.join("cutcraft.json")).unwrap();
    let cli = serde_json::from_value(json!({
        "project":dir,"override_locks":false,"dry_run":true,
        "command":{"Command":{"file":"-","base_revision":0}},
        "inputs":{"-":{"type":"project.update","changes":{"name":"temporary"}}}
    }))
    .unwrap();
    assert_eq!(app::execute(cli).unwrap()["project"]["name"], "temporary");
    assert_eq!(std::fs::read(dir.join("cutcraft.json")).unwrap(), before);
    assert!(Store::open(&dir).unwrap().history().unwrap().is_empty());
}
