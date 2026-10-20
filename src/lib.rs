pub mod edit;
pub mod media;
pub mod media_process;
pub mod progress;
pub mod project;
pub mod query;
pub mod render;
pub mod schema;
pub mod store;
pub mod time;
pub mod visual;

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
