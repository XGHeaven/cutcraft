use cutcraft::{
    id, project,
    query::{self, AssetFilter, ItemFilter},
    store::Store,
    time::Time,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn range(start: i64, duration: i64) -> Value {
    json!({"start":{"num":start,"den":1},"duration":{"num":duration,"den":1}})
}
struct Fixture {
    p: Value,
    asset: String,
    unused: String,
    lut: String,
    font: String,
    child: String,
    first: String,
    second: String,
}
fn fixture() -> Fixture {
    let mut p = project::new("Discovery");
    let root = p["rootSequenceId"].as_str().unwrap().to_string();
    let asset = id("asset");
    let unused = id("asset");
    let lut = id("asset");
    let font = id("asset");
    let child = id("seq");
    let first = id("item");
    let second = id("item");
    p["assets"][&asset] = json!({"name":"喜字 Photo.PNG","kind":"image","uri":"../%E4%BA%91%E5%8D%97/Wedding%20Photos/photo.png","storage":"external","availability":"missing","tags":["Wedding"],"streams":[{"id":"stream_i0","kind":"image","index":0,"video":{"width":100,"height":100,"pixelAspect":{"num":1,"den":1},"colorSpace":"srgb"}}]});
    p["assets"][&unused] = p["assets"][&asset].clone();
    p["assets"][&unused]["uri"] = json!("assets/other.png");
    for (aid, kind) in [(&lut, "lut"), (&font, "font")] {
        p["assets"][aid] = json!({"name":kind,"kind":kind,"uri":format!("assets/{kind}"),"storage":"managed","availability":"missing","streams":[]});
    }
    p["sequences"][&child] = p["sequences"][&root].clone();
    p["sequences"][&child]["name"] = json!("Nested wedding");
    p["sequences"][&child]["durationMode"] = json!("fixed");
    p["sequences"][&child]["duration"] = json!({"num":2,"den":1});
    let effect = json!({"id":id("effect"),"effectType":"builtin.lut","version":"1.0.0","enabled":false,"parameters":{"assetId":lut,"inputColorSpace":"srgb","outputColorSpace":"srgb","interpolation":"tetrahedral","strength":1.0}});
    let mut effect2 = effect.clone();
    effect2["id"] = json!(id("effect"));
    p["sequences"][&child]["tracks"] = json!([{"id":id("track"),"name":"Photos","kind":"video","overlapPolicy":"forbid","items":[
        {"id":first,"type":"media","name":"Opening","assetId":asset,"streamId":"stream_i0","timelineRange":range(0,1),"effects":[effect,effect2]},
        {"id":second,"type":"media","assetId":asset,"streamId":"stream_i0","timelineRange":range(1,1),"enabled":false}
    ]}]);
    p["sequences"][&root]["tracks"] = json!([{"id":id("track"),"name":"Main","kind":"video","overlapPolicy":"forbid","items":[
        {"id":id("item"),"type":"caption","text":"Welcome 喜字","timelineRange":range(0,1),"style":{"fontAssetId":font}},
        {"id":id("item"),"type":"generator","generatorType":"builtin.text","version":"1.0.0","parameters":{"text":"Wedding title","fontAssetId":font},"timelineRange":range(1,1)},
        {"id":id("item"),"type":"sequence","sequenceId":child,"timelineRange":range(10,2),"sourceRange":range(0,2)}
    ]}]);
    let caption = p["sequences"][&root]["tracks"][0]["items"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    p["sequences"][&root]["tracks"].as_array_mut().unwrap().push(json!({
        "id":id("track"),"name":"Captions","kind":"subtitle","overlapPolicy":"forbid","items":[caption]
    }));
    project::validate(&p).unwrap();
    Fixture {
        p,
        asset,
        unused,
        lut,
        font,
        child,
        first,
        second,
    }
}

#[test]
fn search_names_decoded_missing_paths_tags_and_combined_filters() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut filter = AssetFilter {
        query: Some("PHOTO".into()),
        ..Default::default()
    };
    assert_eq!(query::assets(&f.p, dir.path(), &filter)["count"], 2);
    filter.path = Some("云南/Wedding Photos".into());
    let found = query::assets(&f.p, dir.path(), &filter);
    assert_eq!(found["count"], 1);
    assert_eq!(found["assets"][0]["assetId"], f.asset);
    assert_eq!(found["assets"][0]["streams"][0]["id"], "stream_i0");
    let path_query = format!("云南{}Wedding Photos", std::path::MAIN_SEPARATOR);
    let by_path = AssetFilter {
        path: Some(path_query.clone()),
        ..Default::default()
    };
    assert_eq!(query::assets(&f.p, dir.path(), &by_path)["count"], 1);
    let by_item_path = ItemFilter {
        query: Some(path_query),
        ..Default::default()
    };
    assert_eq!(
        query::items(&f.p, dir.path(), &by_item_path).unwrap()["count"],
        2
    );
    filter.tag = Some("wedding".into());
    assert_eq!(query::assets(&f.p, dir.path(), &filter)["count"], 1);
    filter.kind = Some("lut".into());
    assert_eq!(query::assets(&f.p, dir.path(), &filter)["count"], 0);
    let unused = query::assets(
        &f.p,
        dir.path(),
        &AssetFilter {
            unused: true,
            ..Default::default()
        },
    );
    assert_eq!(unused["count"], 1);
    assert_eq!(unused["assets"][0]["assetId"], f.unused);
    assert_eq!(
        query::assets(
            &f.p,
            dir.path(),
            &AssetFilter {
                query: Some(".*".into()),
                ..Default::default()
            }
        )["count"],
        0
    );
}

#[test]
fn usage_counts_are_per_item_include_disabled_effects_fonts_and_nested_sequences() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let usages = query::asset_usages(&f.p, &f.asset).unwrap();
    assert_eq!(usages["count"], 2);
    assert_eq!(usages["usages"][0]["sequenceId"], f.child);
    assert_eq!(usages["usages"][0]["timelineRange"], range(0, 1));
    assert_eq!(usages["usages"][1]["itemEnabled"], false);
    let lut = query::asset_usages(&f.p, &f.lut).unwrap();
    assert_eq!(lut["count"], 1);
    assert_eq!(lut["usages"][0]["references"].as_array().unwrap().len(), 2);
    assert_eq!(
        query::asset_info(&f.p, dir.path(), &f.lut).unwrap()["usageCount"],
        1
    );
    assert_eq!(query::asset_usages(&f.p, &f.font).unwrap()["count"], 2);
    let parents = query::sequence_usages(&f.p, &f.child).unwrap();
    assert_eq!(parents["count"], 1);
    assert_eq!(parents["usages"][0]["timelineRange"], range(10, 2));
    assert_eq!(parents["usages"][0]["isRootSequence"], true);
    assert_eq!(query::asset_usages(&f.p, &f.unused).unwrap()["count"], 0);
}

#[test]
fn item_search_local_exact_time_bounds_and_reference_navigation() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let mut filter = ItemFilter {
        asset: Some(f.asset.clone()),
        ..Default::default()
    };
    assert_eq!(query::items(&f.p, dir.path(), &filter).unwrap()["count"], 2);
    filter.sequence = Some(f.child.clone());
    filter.at = Some("1".parse().unwrap());
    let result = query::items(&f.p, dir.path(), &filter).unwrap();
    assert_eq!(result["count"], 1);
    assert_eq!(result["items"][0]["itemId"], f.second);
    filter.at = Some("2".parse().unwrap());
    assert_eq!(query::items(&f.p, dir.path(), &filter).unwrap()["count"], 0);
    filter.at = Some("999999999/1000000000".parse().unwrap());
    assert_eq!(
        query::items(&f.p, dir.path(), &filter).unwrap()["items"][0]["itemId"],
        f.first
    );
    let paths = query::items(
        &f.p,
        dir.path(),
        &ItemFilter {
            query: Some("Wedding Photos".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(paths["count"], 2);
    let captions = query::items(
        &f.p,
        dir.path(),
        &ItemFilter {
            query: Some("welcome".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(captions["count"], 1);
    assert_eq!(
        query::item_info(&f.p, &f.first).unwrap()["item"]["assetId"],
        f.asset
    );
    assert_eq!(query::sequences(&f.p, Some("nested"))["count"], 1);
    assert_eq!(
        query::tracks(&f.p, Some(&f.child), Some("photos")).unwrap()["count"],
        1
    );
}

#[test]
fn unknown_exact_ids_and_ambiguous_time_scope_are_errors() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    assert!(query::asset_info(&f.p, dir.path(), "missing").is_err());
    assert!(query::asset_usages(&f.p, "missing").is_err());
    assert!(query::item_info(&f.p, "missing").is_err());
    assert!(query::sequence_usages(&f.p, "missing").is_err());
    for filter in [
        ItemFilter {
            asset: Some("missing".into()),
            ..Default::default()
        },
        ItemFilter {
            sequence: Some("missing".into()),
            ..Default::default()
        },
        ItemFilter {
            track: Some("missing".into()),
            ..Default::default()
        },
        ItemFilter {
            at: Some(Time::integer(1)),
            ..Default::default()
        },
        ItemFilter {
            sequence: Some(f.child.clone()),
            track: Some(
                f.p["sequences"][f.p["rootSequenceId"].as_str().unwrap()]["tracks"][0]["id"]
                    .as_str()
                    .unwrap()
                    .into(),
            ),
            ..Default::default()
        },
    ] {
        assert!(query::items(&f.p, dir.path(), &filter).is_err());
    }
}

fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .arg("-p")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn cli_search_aliases_and_queries_leave_snapshot_and_history_unchanged() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let s = Store::init(dir.path(), "Query fixture").unwrap();
    let mut commands = vec![];
    for (aid, a) in f.p["assets"].as_object().unwrap() {
        commands.push(json!({"type":"asset.put","assetId":aid,"asset":a}));
    }
    for (sid, seq) in f.p["sequences"].as_object().unwrap() {
        commands.push(json!({"type":"sequence.put","sequenceId":sid,"sequence":seq}));
    }
    commands
        .push(json!({"type":"project.update","changes":{"rootSequenceId":f.p["rootSequenceId"]}}));
    s.command(json!({"type":"batch","commands":commands}), false, false)
        .unwrap();
    drop(s);
    let before = fs::read(dir.path().join("cutcraft.json")).unwrap();
    let history = fs::read(dir.path().join("history.jsonl")).unwrap();
    assert_eq!(
        cli(
            dir.path(),
            &["asset", "search", "喜字", "--path", "Wedding Photos"]
        )["count"],
        1
    );
    assert_eq!(cli(dir.path(), &["asset", "list", "--unused"])["count"], 1);
    assert_eq!(
        cli(dir.path(), &["asset", "info", &f.asset])["asset"]["kind"],
        "image"
    );
    assert_eq!(cli(dir.path(), &["asset", "usages", &f.asset])["count"], 2);
    assert_eq!(cli(dir.path(), &["item", "search", "photo"])["count"], 2);
    assert_eq!(
        cli(
            dir.path(),
            &[
                "item",
                "list",
                "--asset",
                &f.asset,
                "--sequence",
                &f.child,
                "--at",
                "1"
            ]
        )["items"][0]["itemId"],
        f.second
    );
    assert_eq!(
        cli(dir.path(), &["item", "info", &f.first])["item"]["assetId"],
        f.asset
    );
    assert_eq!(cli(dir.path(), &["sequence", "list", "nested"])["count"], 1);
    assert_eq!(
        cli(dir.path(), &["sequence", "usages", &f.child])["count"],
        1
    );
    assert_eq!(
        cli(
            dir.path(),
            &["track", "list", "photos", "--sequence", &f.child]
        )["count"],
        1
    );
    let bad = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .arg("-p")
        .arg(dir.path())
        .args(["item", "list", "--at", "1"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("--sequence"));
    assert_eq!(before, fs::read(dir.path().join("cutcraft.json")).unwrap());
    assert_eq!(history, fs::read(dir.path().join("history.jsonl")).unwrap());
}
