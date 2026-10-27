//! Embedded editor bundle. Rebuild with `npm ci --prefix web && npm run build --prefix web`.
pub fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "/" | "/index.html" => Some((
            "text/html; charset=utf-8",
            include_bytes!("../web/dist/index.html"),
        )),
        "/assets/editor.js" => Some((
            "text/javascript; charset=utf-8",
            include_bytes!("../web/dist/assets/editor.js"),
        )),
        "/assets/editor.css" => Some((
            "text/css; charset=utf-8",
            include_bytes!("../web/dist/assets/editor.css"),
        )),
        _ => None,
    }
}
