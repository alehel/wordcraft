//! Fonts the user adds without rebuilding (#582): `font.add`, `font.list`, `font.remove`; and
//! the fonts the document's text asks for that aren't installed, with the fonts drawn instead
//! (`font.missing`).
//!
//! An added font joins the process-wide font database on top of the bundled and system fonts
//! (never replacing a face already there), so it shows in the font lists, lays out, renders and
//! embeds in PDFs like any other. Where it is kept between runs is [`FontDb::load_user_fonts`]'s
//! folder (desktop); without one it lasts for the session (the web shell stores it itself).

use serde_json::{Value, json};
use wordcraft_fonts::{FontDb, UserFont};
use wordcraft_layout::DocLayout;

use crate::cmd::insert::base64_decode;
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("font.add", "Add Font…", "Home › Font", add)
            .params(r#"{"path"?: string, "data"?: base64, "name"?: string (file name for "data")} — a TTF, OTF, TTC, WOFF or WOFF2 file; none: pick a file"#)
            .pure(),
        CommandSpec::new("font.list", "Added Fonts", "Home › Font", list).params(r#"{"all"?: bool (also every available family)}"#).pure(),
        CommandSpec::new("font.remove", "Remove Added Font", "Home › Font", remove).params(r#"{"family": string}"#).pure(),
        CommandSpec::new("font.missing", "Font Substitution…", "File › Options › Advanced", missing)
            .params(r#"{} → {"missing": [{"font": string, "shownAs": [string] (the families drawn instead), "metricCompatible": bool (its substitute measures like the font)}]} — the text the current view shows"#)
            .pure(),
    ]
}

fn font_json(f: &UserFont) -> Value {
    json!({
        "file": f.file,
        "families": f.families(),
        "faces": f.faces.iter().map(|(family, style)| json!({"family": family, "style": style})).collect::<Vec<_>>(),
        "path": f.path.as_ref().map(|p| p.to_string_lossy().to_string()),
    })
}

/// Read a font file named by the user, refusing one larger than the cap before reading it.
#[cfg(not(target_arch = "wasm32"))]
fn read_font_file(path: &str) -> Result<Vec<u8>, CmdError> {
    // Through `io`, which keeps automation inside its read root (#769).
    crate::io::read_file_capped(path, wordcraft_fonts::MAX_FONT_FILE_BYTES as u64, "font file").map_err(CmdError::Failed)
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    if p::str(v, "url").is_some() {
        return Err(CmdError::Failed(
            "adding a font from a link isn't available yet: download the font file and add it with {\"path\"} or {\"data\"}".into(),
        ));
    }
    let (name, bytes) = if let Some(d) = p::str(v, "data") {
        // Base64 is a third larger than the file: refuse before decoding an oversized one.
        if d.len() / 4 * 3 > wordcraft_fonts::MAX_FONT_FILE_BYTES + 3 {
            return Err(CmdError::Failed(format!("the font file is larger than {} MB", wordcraft_fonts::MAX_FONT_FILE_BYTES >> 20)));
        }
        let bytes = base64_decode(d).ok_or_else(|| CmdError::Params("bad base64 data".into()))?;
        (p::str(v, "name").unwrap_or("font").to_string(), bytes)
    } else if let Some(path) = p::str(v, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            (path.to_string(), read_font_file(path)?)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            return Err(CmdError::Failed("reading files by path isn't available on the web".into()));
        }
    } else {
        // From the ribbon: the UI shows a file picker and runs this again with the file.
        s.ui_requests.push(json!({"open": "addFont"}));
        return Ok(json!({"pending": true}));
    };
    let r = FontDb::global().install_font(&name, &bytes).map_err(|e| CmdError::Failed(e.to_string()))?;
    // Text already set in the family (shown in a fallback until now) takes the new font.
    s.relayout();
    let families = r.font.families().join(", ");
    s.status = match &r.warning {
        Some(w) => format!("Added {families} ({w})"),
        None => format!("Added {families}"),
    };
    let mut out = font_json(&r.font);
    if let Some(w) = r.warning {
        out["warning"] = json!(w);
    }
    Ok(out)
}

fn list(_: &mut Session, v: &Value) -> CmdResult {
    let db = FontDb::global();
    let mut out = json!({
        "fonts": db.user_fonts().iter().map(font_json).collect::<Vec<_>>(),
        "folder": db.user_font_dir().map(|p| p.to_string_lossy().to_string()),
    });
    if p::bool(v, "all") == Some(true) {
        out["families"] = json!(db.families());
    }
    Ok(out)
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let family = p::str(v, "family").filter(|f| !f.trim().is_empty()).ok_or_else(|| CmdError::Params("family is required".into()))?;
    let gone = FontDb::global().remove_user_font(family.trim()).map_err(|e| CmdError::Failed(e.to_string()))?;
    s.relayout();
    s.status = format!("Removed {}", family.trim());
    Ok(json!({"removed": gone.iter().map(font_json).collect::<Vec<_>>()}))
}

/// A font the document's text asks for that isn't installed.
#[derive(Clone, Debug, PartialEq)]
pub struct MissingFont {
    /// The name the document asks for.
    pub font: String,
    /// The families its text is drawn in instead: the substitute, and fallback faces for
    /// characters the substitute lacks.
    pub shown_as: Vec<String>,
    /// The font's substitute measures like it (Carlito for Calibri…): line breaks don't move.
    /// Fallback faces for characters the substitute lacks don't count: the font itself might
    /// lack them too (an emoji in Calibri text).
    pub metric_compatible: bool,
}

/// The family shown for text set in `font` when it isn't installed (None when it is).
pub fn shown_as(font: &str) -> Option<String> {
    if font.trim().is_empty() {
        return None;
    }
    let r = wordcraft_fonts::word::resolve(font, false, false);
    r.substituted.then(|| r.face.get().family.clone())
}

/// Whether text in `font` drawn in `substitute` is worth a warning: the substitute measures
/// differently, so line breaks can move, and the font isn't a symbol font (drawn as its
/// Unicode equivalents on purpose, which layout leaves out of [`missing_fonts`] too). The one
/// rule the font box and the status bar share.
pub fn warns(font: &str, substitute: &str) -> bool {
    !wordcraft_doc::symbol_fonts::is_symbol_font(font) && !wordcraft_fonts::word::metric_compatible(font, substitute)
}

/// [`shown_as`] when it [`warns`]: what the font box warns about.
pub fn layout_changing_substitute(font: &str) -> Option<String> {
    shown_as(font).filter(|s| warns(font, s))
}

/// The fonts the laid-out text asks for that aren't installed, from what layout drew
/// ([`DocLayout::substituted_fonts`]): only text the current view shows counts (no headers or
/// footers in Web, Draft or Read view), in every story on its pages, list labels and field
/// results included. Names differing only in case are one font.
pub fn missing_fonts(layout: &DocLayout) -> Vec<MissingFont> {
    let mut out: Vec<MissingFont> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (font, face) in layout.substituted_fonts() {
        let family = face.get().family.clone();
        let font = font.trim();
        match index.get(&font.to_lowercase()).and_then(|&i| out.get_mut(i)) {
            Some(m) => {
                if !m.shown_as.contains(&family) {
                    m.shown_as.push(family);
                }
            }
            None => {
                index.insert(font.to_lowercase(), out.len());
                let metric_compatible = shown_as(font).is_some_and(|s| !warns(font, &s));
                out.push(MissingFont { font: font.to_string(), shown_as: vec![family], metric_compatible });
            }
        }
    }
    out.sort_by_cached_key(|m| m.font.to_lowercase());
    out
}

fn missing(s: &mut Session, _: &Value) -> CmdResult {
    let fonts: Vec<Value> =
        missing_fonts(&s.layout()).iter().map(|f| json!({"font": f.font, "shownAs": f.shown_as, "metricCompatible": f.metric_compatible})).collect();
    Ok(json!({"missing": fonts}))
}
