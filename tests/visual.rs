use cutcraft::{
    id, project, render,
    visual::{Cube, Geometry, Pipeline, Space},
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}
#[test]
fn cube_interpolation_domains_and_all_tetrahedra() {
    let cube=Cube::parse("LUT_3D_SIZE 2\nDOMAIN_MIN -1 -1 -1\nDOMAIN_MAX 1 1 1\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n1 1 1\n").unwrap();
    for rgb in [
        [-0.6, -0.2, 0.2],
        [-0.6, 0.2, -0.2],
        [-0.2, -0.6, 0.2],
        [-0.2, 0.2, -0.6],
        [0.2, -0.6, -0.2],
        [0.2, -0.2, -0.6],
    ] {
        close(cube.sample(rgb, false)[0], 0.048);
        close(cube.sample(rgb, true)[0], 0.2);
    }
    close(cube.sample([1.; 3], true)[0], 1.);
    close(cube.sample([-9.; 3], false)[0], 0.);
    let one = Cube::parse("\u{feff}TITLE \"# literal\" # comment\nLUT_1D_SIZE 2\n0 1 0\n1 0 2\n")
        .unwrap();
    assert_eq!(one.sample([0.25, 0.5, 0.75], false), [0.25, 0.5, 1.5]);
    assert!(one.check_interpolation("tetrahedral").is_err());
}
#[test]
fn cube_rejects_ambiguous_and_broken_inputs() {
    for input in [
        "",
        "LUT_1D_SIZE 1",
        "LUT_1D_SIZE 2\nLUT_3D_SIZE 2",
        "LUT_1D_SIZE 2\n0 0 0",
        "LUT_1D_SIZE 2\n0 0 0\n1 1 NaN",
        "LUT_1D_SIZE 2\nDOMAIN_MIN 1 1 1\n0 0 0\n1 1 1",
        "LUT_1D_SIZE 2\n0 0 0\n1 1 1\n2 2 2",
        "LUT_1D_SIZE 2\n0 0 0\nTITLE \"late\"\n1 1 1",
        "UNKNOWN 2\n0 0 0",
    ] {
        assert!(Cube::parse(input).is_err(), "accepted {input}");
    }
}
fn effect(kind: &str, p: Value) -> Value {
    json!({"id":id("effect"),"effectType":kind,"version":"1.0.0","parameters":p})
}
#[test]
fn linear_color_math_order_and_curve_segments() {
    let temp = tempfile::tempdir().unwrap();
    let p = project::new("color");
    let item = json!({"effects":[effect("builtin.color-grade",json!({"exposureEv":1})),effect("builtin.curves",json!({"rgb":[{"x":0,"y":0},{"x":0.5,"y":0.25},{"x":1,"y":1}]}))]});
    let pipe = Pipeline::new(&item, &p, temp.path(), Space::Linear).unwrap();
    let out = pipe.pixel([0.125, 0.25, 0.375]);
    for (got, linear) in out.into_iter().zip([0.125, 0.25, 0.625]) {
        close(got, Space::Rec709.encode(linear));
    }
    for space in [Space::Linear, Space::Srgb, Space::Rec709] {
        for x in [0., 0.001, 0.1, 0.5, 1.] {
            close(space.decode(space.encode(x)), x);
        }
    }
}
fn raw(pixels: &[[u16; 4]]) -> Vec<u8> {
    pixels
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}
#[test]
fn transforms_rotate_mirror_crop_sar_and_preserve_alpha() {
    let src = raw(&[
        [65535, 0, 0, 65535],
        [0, 65535, 0, 65535],
        [0, 0, 65535, 65535],
        [65535, 65535, 65535, 65535],
    ]);
    let mut out = vec![0; 32];
    Geometry::new(2, 2, 1., 2, 2, &json!({"rotationDegrees":90}))
        .unwrap()
        .frame(&src, None, &mut out);
    assert_eq!(
        out,
        raw(&[
            [0, 0, 65535, 65535],
            [65535, 0, 0, 65535],
            [65535, 65535, 65535, 65535],
            [0, 65535, 0, 65535]
        ])
    );
    Geometry::new(2, 2, 1., 2, 2, &json!({"scale":{"x":-1,"y":1}}))
        .unwrap()
        .frame(&src, None, &mut out);
    assert_eq!(out[..8], src[8..16]);
    Geometry::new(2, 2, 1., 2, 2, &json!({"scale":{"x":0}}))
        .unwrap()
        .frame(&src, None, &mut out);
    assert!(out.iter().all(|v| *v == 0));
    let mut wide = vec![0; 4 * 2 * 8];
    Geometry::new(2, 2, 2., 4, 2, &json!({}))
        .unwrap()
        .frame(&src, None, &mut wide);
    assert!(wide.chunks_exact(8).all(|p| p[6] == 255 && p[7] == 255));
    let transparent = raw(&[[65535, 0, 0, 0], [0, 65535, 0, 65535]]);
    let mut enlarged = vec![0; 4 * 8];
    Geometry::new(2, 1, 1., 4, 1, &json!({"scale":{"x":2}}))
        .unwrap()
        .frame(&transparent, None, &mut enlarged);
    assert!(
        enlarged.chunks_exact(8).all(|p| p[0] == 0 && p[1] == 0),
        "transparent red leaked"
    );
}
fn render_fixture() -> Value {
    let mut p = project::new("visual");
    let sid = p["rootSequenceId"].as_str().unwrap().to_string();
    p["sequences"][&sid]["format"]["width"] = json!(32);
    p["sequences"][&sid]["format"]["height"] = json!(32);
    p["sequences"][&sid]["tracks"] = json!([{"id":id("track"),"kind":"video","name":"V","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"generator","generatorType":"builtin.color","parameters":{"color":{"r":0,"g":1,"b":0,"a":1}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":10}}}]}]);
    p
}
fn pixel(path: &Path, x: usize, y: usize) -> [u8; 3] {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let i = (y * 32 + x) * 3;
    output.stdout[i..i + 3].try_into().unwrap()
}
#[test]
fn actual_render_lut_strength_transform_and_cli_import() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        assert!(std::env::var_os("CUTCRAFT_REQUIRE_FFMPEG").is_none());
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let lut = tmp.path().join("look ' 色.cube");
    std::fs::write(&lut, "LUT_1D_SIZE 2\n1 0 0\n1 0 0\n").unwrap();
    let mut p = render_fixture();
    let sid = p["rootSequenceId"].as_str().unwrap().to_string();
    let aid = id("asset");
    p["assets"][&aid] = cutcraft::media::asset(
        &lut,
        url::Url::from_file_path(&lut).unwrap().as_str(),
        false,
    )
    .unwrap();
    let item = &mut p["sequences"][&sid]["tracks"][0]["items"][0];
    item["effects"] = json!([effect(
        "builtin.lut",
        json!({"assetId":aid,"strength":0.5,"inputColorSpace":"linear-srgb","outputColorSpace":"linear-srgb","interpolation":"linear"})
    )]);
    item["video"] = json!({"transform":{"scale":{"x":0.5,"y":0.5}}});
    let output = tmp.path().join("out.mp4");
    render::render(&p, tmp.path(), &sid, &output, false).unwrap();
    let native = cutcraft::media::probe(&output).unwrap();
    assert_eq!(native["streams"][0]["color_primaries"], "bt709");
    assert_eq!(native["streams"][0]["color_transfer"], "bt709");
    let rgb = pixel(&output, 16, 16);
    assert!(
        (170..195).contains(&rgb[0]) && (170..195).contains(&rgb[1]) && rgb[2] < 15,
        "{rgb:?}"
    );
    assert!(pixel(&output, 1, 1).iter().all(|v| *v < 8));
    let dir = tmp.path().join("project");
    drop(cutcraft::store::Store::init(&dir, "lut").unwrap());
    let cli = Command::new(env!("CARGO_BIN_EXE_cutcraft"))
        .arg("-p")
        .arg(&dir)
        .arg("import")
        .arg(&lut)
        .arg("--managed")
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let result: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(
        result["project"]["assets"][result["assetId"].as_str().unwrap()]["kind"],
        "lut"
    );
}

#[test]
fn adjustment_layer_and_text_effects_render() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut p = render_fixture();
    let sid = p["rootSequenceId"].as_str().unwrap().to_string();
    let adjustment = json!({"id":id("item"),"type":"adjustment","timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":10}},"effects":[effect("builtin.curves",json!({"r":[{"x":0,"y":1},{"x":1,"y":1}],"g":[{"x":0,"y":0},{"x":1,"y":0}]}))]});
    p["sequences"][&sid]["tracks"].as_array_mut().unwrap().push(json!({"id":id("track"),"kind":"video","name":"Adjustment","overlapPolicy":"forbid","items":[adjustment]}));
    let output = tmp.path().join("adjustment.mp4");
    render::render(&p, tmp.path(), &sid, &output, false).unwrap();
    let rgb = pixel(&output, 16, 16);
    assert!(rgb[0] > 230 && rgb[1] < 15 && rgb[2] < 15, "{rgb:?}");
    let item = &mut p["sequences"][&sid]["tracks"][1]["items"][0];
    item["type"] = json!("generator");
    item["generatorType"] = json!("builtin.text");
    item["parameters"] = json!({"text":"X","fontSizePixels":20,"position":"center"});
    item["video"] = json!({"transform":{"scale":{"x":1,"y":1}}});
    let output = tmp.path().join("text.mp4");
    render::render(&p, tmp.path(), &sid, &output, false).unwrap();
    // Check inside the unscaled text canvas, outside glyphs. A corner outside a
    // shrunken canvas would pass even if the entire canvas were opaque.
    let corner = pixel(&output, 1, 1);
    assert!(corner[1] > 230 && corner[0] < 15, "{corner:?}");
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(output)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output
            .stdout
            .chunks_exact(3)
            .any(|p| p[0] > 150 && p[1] < 100),
        "effect did not color text"
    );
}

#[test]
fn cli_transform_effect_undo_and_invalid_lut_are_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let store = cutcraft::store::Store::init(tmp.path(), "cli").unwrap();
    let p = render_fixture();
    let sid = p["rootSequenceId"].as_str().unwrap();
    let item = p["sequences"][sid]["tracks"][0]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    store.command(json!({"type":"batch","commands":[{"type":"sequence.put","sequenceId":sid,"sequence":p["sequences"][sid]},{"type":"project.update","changes":{"rootSequenceId":sid}}]}),false,false).unwrap();
    drop(store);
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cutcraft"))
            .arg("-p")
            .arg(tmp.path())
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&[
        "item",
        "transform",
        &item,
        "--x",
        "0.6",
        "--scale-x",
        "-1",
        "--rotation",
        "30",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run(&[
        "item",
        "effect",
        &item,
        "--kind",
        "color-grade",
        "--parameters",
        r#"{"exposureEv":1}"#,
    ]);
    assert!(output.status.success());
    let store = cutcraft::store::Store::open(tmp.path()).unwrap();
    let p = store.project().unwrap();
    let v = &p["sequences"][sid]["tracks"][0]["items"][0];
    assert_eq!(v["video"]["transform"]["scale"]["y"], 1.);
    assert_eq!(v["effects"][0]["effectType"], "builtin.color-grade");
    drop(store);
    assert!(run(&["undo"]).status.success());
    let before = std::fs::read(tmp.path().join("cutcraft.json")).unwrap();
    let bad = tmp.path().join("bad.cube");
    std::fs::write(&bad, "LUT_3D_SIZE 2\n0 0 0\n").unwrap();
    assert!(!run(&["import", bad.to_str().unwrap(), "--managed"])
        .status
        .success());
    assert_eq!(
        before,
        std::fs::read(tmp.path().join("cutcraft.json")).unwrap()
    );
}

#[test]
fn nested_sequence_preserves_alpha_and_accepts_parent_effects() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut p = render_fixture();
    let sid = p["rootSequenceId"].as_str().unwrap().to_string();
    let child = id("seq");
    p["sequences"][&child] = p["sequences"][&sid].clone();
    p["sequences"][&child]["tracks"][0]["id"] = json!(id("track"));
    p["sequences"][&child]["tracks"][0]["items"][0]["id"] = json!(id("item"));
    p["sequences"][&child]["background"] = json!({"r":0,"g":0,"b":0,"a":0});
    p["sequences"][&child]["tracks"][0]["items"][0]["video"] =
        json!({"transform":{"scale":{"x":0.5,"y":0.5}}});
    p["sequences"][&sid]["tracks"][0]["items"][0]["parameters"]["color"] =
        json!({"r":0,"g":0,"b":1,"a":1});
    p["sequences"][&sid]["tracks"].as_array_mut().unwrap().push(json!({"id":id("track"),"kind":"video","name":"Nested","overlapPolicy":"forbid","items":[{"id":id("item"),"type":"sequence","sequenceId":child,"sourceRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":10}},"timelineRange":{"start":{"num":0,"den":1},"duration":{"num":1,"den":10}},"effects":[effect("builtin.curves",json!({"r":[{"x":0,"y":1},{"x":1,"y":1}],"g":[{"x":0,"y":0},{"x":1,"y":0}]}))]}]}));
    let out = tmp.path().join("nested.mp4");
    render::render(&p, tmp.path(), &sid, &out, false).unwrap();
    let center = pixel(&out, 16, 16);
    assert!(center[0] > 230 && center[1] < 15, "{center:?}");
    let corner = pixel(&out, 1, 1);
    assert!(corner[2] > 230 && corner[0] < 15, "lost alpha: {corner:?}");
}
