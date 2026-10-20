use cutcraft::{
    animation as a, edit, id,
    project::{self, text},
    time::{range, Time},
    timeline,
};
use serde_json::{json, Value};
fn t(n: i64) -> Value {
    json!({"num":n,"den":1})
}
fn curve(kind: &str, keys: Value) -> Value {
    json!({"$animation":{"version":"1.0.0","valueType":kind,"timeReference":"owner","keyframes":keys}})
}
fn numeric() -> Value {
    curve(
        "number",
        json!([{"offset":t(0),"value":0},{"offset":t(2),"value":1}]),
    )
}
fn remap() -> Value {
    curve(
        "time",
        json!([{"offset":t(0),"value":t(0),"easing":{"type":"cubic-hermite","outRate":{"num":1,"den":2},"inRate":{"num":3,"den":2}}},{"offset":t(2),"value":t(2)},{"offset":t(3),"value":t(2)},{"offset":t(5),"value":t(0)}]),
    )
}
#[test]
fn exact_retime_and_boundary_direction() {
    let map = remap();
    a::validate(&map, &Time::integer(5)).unwrap();
    assert_eq!(
        a::time_value(&map, &Time::integer(1), false).unwrap().0,
        Time::new(3, 4).unwrap()
    );
    let item = json!({"type":"sequence","sourceRange":{"start":t(10),"duration":t(3)},"timelineRange":{"start":t(0),"duration":t(5)},"playback":{"mode":"remap","version":"1.0.0","timeMap":map}});
    a::validate_item(&item, "video").unwrap();
    let (src, upper, frozen) = a::mapping(&item, &Time::new(5, 2).unwrap(), false).unwrap();
    assert_eq!(src, Time::integer(12));
    assert!(!upper && frozen);
    let (src, upper, _) = a::mapping(&item, &Time::integer(4), false).unwrap();
    assert_eq!(src, Time::integer(11));
    assert!(upper);
    // Endpoint handle uses tangent, never extends the cubic polynomial.
    assert_eq!(
        a::mapping(&item, &Time::integer(-1), false).unwrap().0,
        Time::new(19, 2).unwrap()
    );
    let mut bad = item.clone();
    bad["playback"]["timeMap"]["$animation"]["keyframes"][0]["easing"]["outRate"] = t(100);
    assert!(a::validate_item(&bad, "video").is_err());
    assert!(a::validate_item(&item, "audio").is_err());
}
#[test]
fn hold_bezier_color_and_opaque_slots() {
    let mut n = numeric();
    assert_eq!(a::evaluate(&n, &Time::integer(1)).unwrap(), json!(0.5));
    n["$animation"]["keyframes"][0]["easing"] = json!({"type":"hold"});
    assert_eq!(a::evaluate(&n, &Time::integer(1)).unwrap(), json!(0));
    assert_eq!(a::evaluate(&n, &Time::integer(2)).unwrap(), json!(1));
    n["$animation"]["keyframes"][0]["easing"] =
        json!({"type":"cubic-bezier","x1":0.1,"x2":0.2,"y1":0.8,"y2":0.9});
    assert!(
        a::evaluate(&n, &Time::integer(1))
            .unwrap()
            .as_f64()
            .unwrap()
            > 0.85
    );
    let mut c = curve(
        "color",
        json!([{"offset":t(0),"value":{"r":1,"g":0,"b":0,"a":0}},{"offset":t(2),"value":{"r":0,"g":0,"b":1,"a":1}}]),
    );
    c["$animation"]["colorSpace"] = json!("linear-srgb");
    assert_eq!(
        a::evaluate(&c, &Time::integer(1)).unwrap(),
        json!({"r":0.,"g":0.,"b":1.,"a":0.5})
    );
    let item = json!({"video":{"opacity":numeric()},"metadata":{"$animation":"opaque"},"effects":[{"effectType":"third.party","parameters":{"x":numeric()}}]});
    let evaluated = a::evaluated(&item, &Time::integer(1)).unwrap();
    assert_eq!(evaluated["metadata"], item["metadata"]);
    assert_eq!(evaluated["effects"], item["effects"]);
    assert_eq!(evaluated["video"]["opacity"], 0.5);
}
#[test]
fn draft_projects_and_invalid_semantics() {
    for f in [
        include_str!("../spec/archive/0.4.0/examples/animated.cutcraft.json"),
        include_str!("../spec/archive/0.4.0/examples/transition.cutcraft.json"),
        include_str!("../spec/archive/0.4.0/examples/retime.cutcraft.json"),
    ] {
        project::validate(&serde_json::from_str(f).unwrap()).unwrap();
    }
    let mut n = numeric();
    n["$animation"]["keyframes"][1]["offset"] = t(0);
    assert!(a::validate(&n, &Time::integer(2)).is_err());
    let mut n = numeric();
    n["$animation"]["keyframes"][1]["easing"] = json!({"type":"linear"});
    assert!(a::validate(&n, &Time::integer(2)).is_err());
}
#[test]
fn speed_is_atomic_linked_and_undoable() {
    let mut p = project::new("speed");
    let sid = text(&p["rootSequenceId"]).to_string();
    let child = id("seq");
    p["sequences"][&child] = p["sequences"][&sid].clone();
    p["sequences"][&child]["durationMode"] = json!("fixed");
    p["sequences"][&child]["duration"] = t(10);
    let iid = id("item");
    let aid = id("item");
    let lid = id("link");
    let vtrack = id("track");
    let atrack = id("track");
    let mut v = json!({"id":iid,"type":"sequence","sequenceId":child,"timelineRange":range(&Time::zero(),&Time::integer(4)).unwrap(),"sourceRange":range(&Time::zero(),&Time::integer(4)).unwrap(),"linkGroupId":lid,"video":{"opacity":numeric()}});
    let mut audio = v.clone();
    audio["id"] = json!(aid);
    audio.as_object_mut().unwrap().remove("video");
    v["video"]["opacity"]["$animation"]["keyframes"][1]["offset"] = t(4);
    p["sequences"][&sid]["tracks"] = json!([{"id":vtrack,"kind":"video","name":"V","overlapPolicy":"forbid","items":[v]},{"id":atrack,"kind":"audio","name":"A","overlapPolicy":"forbid","items":[audio]}]);
    p["sequences"][&sid]["links"] = json!([{"id":lid,"kind":"av","itemIds":[iid,aid]}]);
    project::validate(&p).unwrap();
    let cmd = timeline::retime(
        &p,
        &sid,
        &iid,
        None,
        Some("2"),
        "none",
        "stretch",
        false,
        Some("mute"),
    )
    .unwrap();
    let (next, event) = edit::apply(&p, &edit::operation(&p, cmd), false).unwrap();
    let tracks = &next["sequences"][&sid]["tracks"];
    assert_eq!(tracks[0]["items"][0]["timelineRange"]["duration"], t(2));
    assert_eq!(tracks[1]["items"][0]["timelineRange"]["duration"], t(2));
    assert_eq!(
        tracks[0]["items"][0]["video"]["opacity"]["$animation"]["keyframes"][1]["offset"],
        t(2)
    );
    let (undo, _) = edit::apply(
        &next,
        &edit::operation(
            &next,
            json!({"type":"batch","commands":event["inverseCommands"]}),
        ),
        false,
    )
    .unwrap();
    assert_eq!(undo["sequences"], p["sequences"]);
    let cmd = timeline::retime(
        &p,
        &sid,
        &iid,
        None,
        Some("2"),
        "none",
        "keep-local",
        false,
        Some("mute"),
    )
    .unwrap();
    assert!(edit::apply(&p, &edit::operation(&p, cmd), false).is_err());
}

#[test]
fn slicing_preserves_bezier_hermite_and_hold_functions() {
    let mut bezier = numeric();
    bezier["$animation"]["keyframes"][0]["easing"] =
        json!({"type":"cubic-bezier","x1":0.2,"y1":0.05,"x2":0.7,"y2":0.95});
    let start = Time::new(1, 3).unwrap();
    let end = Time::new(5, 3).unwrap();
    let clipped = a::slice(&bezier, &start, &end).unwrap();
    for n in 0..41 {
        let local = end.sub(&start).mul(&Time::new(n, 40).unwrap());
        let original = a::evaluate(&bezier, &start.add(&local))
            .unwrap()
            .as_f64()
            .unwrap();
        let actual = a::evaluate(&clipped, &local).unwrap().as_f64().unwrap();
        assert!((original - actual).abs() < 1e-10, "{original} != {actual}");
    }
    let map = remap();
    let clipped = a::slice(&map, &start, &Time::new(9, 2).unwrap()).unwrap();
    for n in 0..40 {
        let local = Time::new(n, 10).unwrap();
        assert_eq!(
            a::time_value(&map, &start.add(&local), false).unwrap().0,
            a::time_value(&clipped, &local, false).unwrap().0
        );
    }
    let mut hold = numeric();
    hold["$animation"]["keyframes"][0]["easing"] = json!({"type":"hold"});
    let clipped = a::slice(&hold, &Time::integer(1), &Time::integer(3)).unwrap();
    assert_eq!(
        a::evaluate(&clipped, &Time::new(9, 10).unwrap()).unwrap(),
        json!(0)
    );
    assert_eq!(a::evaluate(&clipped, &Time::integer(1)).unwrap(), json!(1));
}

#[test]
fn zero_tangent_handle_holds_and_remap_split_is_preserved() {
    let mut p: Value = serde_json::from_str(include_str!(
        "../spec/archive/0.4.0/examples/retime.cutcraft.json"
    ))
    .unwrap();
    let sid = text(&p["rootSequenceId"]).to_string();
    let item = p["sequences"][&sid]["tracks"][0]["items"][0].clone();
    let iid = text(&item["id"]);
    let op = edit::operation(
        &p,
        json!({"type":"item.split","sequenceId":sid,"itemId":iid,"at":{"num":3,"den":2},"leftItemId":id("item"),"rightItemId":id("item")}),
    );
    let (next, _) = edit::apply(&p, &op, false).unwrap();
    let items = next["sequences"][&sid]["tracks"][0]["items"]
        .as_array()
        .unwrap();
    for n in 0..60 {
        let t = Time::new(n, 10).unwrap();
        let i = if t < Time::new(3, 2).unwrap() {
            &items[0]
        } else {
            &items[1]
        };
        let local = t.sub(&Time::from_json(&i["timelineRange"]["start"]).unwrap());
        assert_eq!(
            a::mapping(&item, &t, false).unwrap().0,
            a::mapping(i, &local, false).unwrap().0
        );
    }
    let map = curve(
        "time",
        json!([{"offset":t(0),"value":t(1),"easing":{"type":"cubic-hermite","outRate":t(0),"inRate":t(1)}},{"offset":t(2),"value":t(2)}]),
    );
    let i = &mut p["sequences"][&sid]["tracks"][0]["items"][0];
    i["playback"]["timeMap"] = map;
    i["timelineRange"]["duration"] = t(2);
    let (_, upper, frozen) = a::mapping(i, &Time::integer(-1), false).unwrap();
    assert!(!upper && frozen);
}
