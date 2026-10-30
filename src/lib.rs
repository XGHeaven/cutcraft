pub mod app;
pub mod cache;
pub mod capabilities;
pub mod daemon;
pub mod doctor;
pub mod edit;
pub mod fonts;
pub mod media;
pub mod media_process;
mod media_stream;
pub mod preview;
pub mod preview_review;
pub mod progress;
pub mod project;
pub mod query;
pub mod range_edit;
pub mod render;
pub mod schema;
pub mod store;
pub mod time;
pub mod visual;
pub mod web;

pub const SPEC_VERSION: &str = "0.1.0";
pub fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::now_v7())
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub mod animation;
pub mod frame_render;
pub mod timeline;

pub mod image_sequence;

pub mod encoding;

pub mod text_card;
