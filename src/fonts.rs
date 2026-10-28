//! Resolve explicit fonts before invoking FFmpeg. Never pass user font names into a filter.
use crate::{
    media,
    project::{array, enabled, text},
};
use anyhow::{bail, ensure, Context, Result};
use fontdb::{Database, Family, Query, Source};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::UNIX_EPOCH,
};

static SYSTEM_FONTS: OnceLock<Database> = OnceLock::new();

pub fn style(item: &Value) -> Option<&Value> {
    if item["type"] == "caption" {
        Some(&item["style"])
    } else if item["type"] == "generator" && item["generatorType"] == "builtin.text" {
        Some(&item["parameters"])
    } else {
        None
    }
}

pub fn content(item: &Value) -> &str {
    if item["type"] == "caption" {
        text(&item["text"])
    } else {
        text(&item["parameters"]["text"])
    }
}

pub fn validate_style(style: &Value) -> Result<()> {
    if let Some(value) = style.get("fontFamily") {
        let name = value
            .as_str()
            .context("FONT_FAMILY_INVALID: expected a string")?;
        ensure!(
            !name.trim().is_empty() && name.chars().count() <= 200,
            "FONT_FAMILY_INVALID: expected 1–200 characters, not only whitespace"
        );
    }
    Ok(())
}

#[derive(Debug)]
pub struct ResolvedFont {
    pub path: PathBuf,
    pub index: u32,
}

fn select(database: &Database, name: &str) -> Option<fontdb::ID> {
    if let Some(face) = database
        .faces()
        .find(|face| face.post_script_name.eq_ignore_ascii_case(name))
    {
        return Some(face.id);
    }
    let family = database
        .faces()
        .flat_map(|face| &face.families)
        .find(|(family, _)| family.eq_ignore_ascii_case(name))?;
    database.query(&Query {
        families: &[Family::Name(&family.0)],
        ..Query::default()
    })
}

pub fn resolve(project: &Value, dir: &Path, style: &Value) -> Result<Option<ResolvedFont>> {
    validate_style(style)?;
    // Preserve the established precedence and face-zero behavior for pinned assets.
    if let Some(id) = style["fontAssetId"].as_str() {
        let asset = &project["assets"][id];
        ensure!(asset["kind"] == "font", "FONT_ASSET_MISSING: {id}");
        return Ok(Some(ResolvedFont {
            path: media::local_path(dir, text(&asset["uri"]))?,
            index: 0,
        }));
    }
    let Some(name) = style["fontFamily"].as_str() else {
        return Ok(None);
    };
    let database = SYSTEM_FONTS.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db
    });
    let id = select(database, name).with_context(|| {
        format!("FONT_NOT_FOUND: {name}; install this font or use --font-asset")
    })?;
    let (source, index) = database.face_source(id).context("FONT_NOT_FOUND")?;
    let path = match source {
        Source::File(path) | Source::SharedFile(path, _) => path,
        Source::Binary(_) => bail!("FONT_INVALID: system font has no file"),
    };
    Ok(Some(ResolvedFont { path, index }))
}

fn needs_glyph(c: char) -> bool {
    !c.is_whitespace()
        && !c.is_control()
        && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}'
            | '\u{e0100}'..='\u{e01ef}')
}

fn check_glyphs(data: &[u8], index: u32, content: &str) -> Result<()> {
    let face =
        ttf_parser::Face::parse(data, index).map_err(|e| anyhow::anyhow!("FONT_INVALID: {e:?}"))?;
    let mut missing = std::collections::BTreeSet::new();
    for c in content.chars().filter(|c| needs_glyph(*c)) {
        if face.glyph_index(c).is_none_or(|id| id.0 == 0) {
            missing.insert(format!("U+{:04X}", c as u32));
            if missing.len() >= 8 {
                break;
            }
        }
    }
    ensure!(
        missing.is_empty(),
        "FONT_GLYPH_MISSING: {}; choose a font covering this text",
        missing.into_iter().collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

impl ResolvedFont {
    pub fn signature(&self) -> Result<Value> {
        let stat = fs::metadata(&self.path)
            .with_context(|| format!("FONT_NOT_FOUND: {}", self.path.display()))?;
        ensure!(stat.is_file(), "FONT_INVALID: not a file");
        let modified = stat
            .modified()?
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|v| (v.as_secs(), v.subsec_nanos()));
        Ok(json!({"path":self.path,"index":self.index,"size":stat.len(),"modified":modified}))
    }
    fn read(&self, content: &str) -> Result<Vec<u8>> {
        let data = fs::read(&self.path)
            .with_context(|| format!("FONT_NOT_FOUND: {}", self.path.display()))?;
        check_glyphs(&data, self.index, content)?;
        Ok(data)
    }
    pub fn validate(&self, content: &str) -> Result<()> {
        self.read(content).map(|_| ())
    }
    pub fn prepare(&self, scratch: &Path, name: &str, content: &str) -> Result<String> {
        let mut data = self.read(content)?;
        if self.index != 0 {
            // drawtext's fontfile opens face zero. In our private TTC copy, point the
            // first directory entry at the selected face. SFNT table offsets stay
            // absolute and all tables stay intact; never modify the system font.
            ensure!(
                ttf_parser::fonts_in_collection(&data).is_some(),
                "FONT_INVALID: expected TTC"
            );
            let start = 12usize
                .checked_add(
                    (self.index as usize)
                        .checked_mul(4)
                        .context("FONT_INVALID: index")?,
                )
                .context("FONT_INVALID: index")?;
            let offset: [u8; 4] = data
                .get(start..start + 4)
                .context("FONT_INVALID: TTC index")?
                .try_into()?;
            data.get_mut(12..16)
                .context("FONT_INVALID: TTC header")?
                .copy_from_slice(&offset);
        }
        fs::write(scratch.join(name), data)?;
        Ok(format!(":fontfile={name}"))
    }
}

fn each_text(project: &Value, mut visit: impl FnMut(&Value, &Value) -> Result<()>) -> Result<()> {
    for seq in project["sequences"]
        .as_object()
        .context("sequences")?
        .values()
    {
        for track in array(&seq["tracks"]).iter().filter(|t| enabled(t)) {
            for item in array(&track["items"]).iter().filter(|i| enabled(i)) {
                if let Some(style) = style(item) {
                    visit(item, style)?;
                }
            }
        }
    }
    Ok(())
}

pub fn validate(project: &Value, dir: &Path) -> Result<()> {
    each_text(project, |item, style| {
        if let Some(font) = resolve(project, dir, style)? {
            font.validate(content(item))?;
        }
        Ok(())
    })
}

/// Include resolved file identity in composed PNG caches, not just the family name.
pub fn signatures(project: &Value, dir: &Path) -> Result<Vec<Value>> {
    let mut signatures = Vec::new();
    each_text(project, |_, style| {
        if let Some(font) = resolve(project, dir, style)? {
            signatures.push(font.signature()?);
        }
        Ok(())
    })?;
    Ok(signatures)
}
