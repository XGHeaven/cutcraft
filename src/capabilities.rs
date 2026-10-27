//! Protocol capability sets are explicit: internal draft numbers are not ordered
//! relative to the first public specification.
#[derive(Clone, Copy)]
pub enum Capability {
    ImageSequences,
    InteractivePreview,
    GaussianBlur,
}

pub fn versions(capability: Capability) -> &'static [&'static str] {
    match capability {
        Capability::ImageSequences => &["0.1.0", "0.5.0"],
        Capability::InteractivePreview => &["0.1.0", "0.4.0", "0.5.0"],
        Capability::GaussianBlur => &["0.1.0"],
    }
}

pub fn supports(version: &str, capability: Capability) -> bool {
    versions(capability).contains(&version)
}
