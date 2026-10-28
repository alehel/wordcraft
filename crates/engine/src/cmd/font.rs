//! Fonts the user adds without rebuilding (#582): `font.add`, `font.list`, `font.remove`; and
//! the document's fonts that aren't installed, with the fonts shown instead (`font.missing`).
//!
//! An added font joins the process-wide font database on top of the bundled and system fonts
//! (never replacing a face already there), so it shows in the font lists, lays out, renders and
//! embeds in PDFs like any other. Where it is kept between runs is [`FontDb::load_user_fonts`]'s
//! folder (desktop); without one it lasts for the session (the web shell stores it itself).

use serde_json::{Value, json};
use std::collections::BTreeMap;

use wordcraft_doc::{Document, StoryRef};
use wordcraft_fonts::{FontDb, UserFont};

use crate::cmd::insert::base64_decode;
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("font.add", "Add Font…", "Home › Font", add)
            .params(r#"{"path"?: string, "data"?: base64, "name"?: string (file name for "data")} — a TTF, OTF, TTC, WOFF or WOFF2 file; none: pick a file"#)
            .pure(),
        CommandSpec::new("font.list", "Added Fonts", "Home › Font", list).params(r#"{"all"?: bool (also every available family)}"#).pure(),
        CommandSpec::new("font.remove", "Remove Added Font", "Home › Font", remove).params(r#"{"family": string}"#).pure(),
        CommandSpec::new("font.missing", "Font Substitution…", "File › Options › Advanced", missing).pure(),
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

/// Most paragraphs [`missing_fonts`] looks at, so a huge document can't stall the status bar.
const MAX_SCANNED_PARAS: usize = 200_000;

/// The family shown for text set in `font` when it isn't installed (None when it is).
pub fn shown_as(font: &str) -> Option<String> {
    if font.trim().is_empty() {
        return None;
    }
    let r = wordcraft_fonts::word::resolve(font, false, false);
    r.substituted.then(|| r.face.get().family.clone())
}

/// The fonts the document's text is set in that aren't installed, each with the family shown
/// instead, by name. Every story counts (headers, notes, text boxes…); a font only a style
/// names, with no text set in it, doesn't.
pub fn missing_fonts(doc: &Document) -> Vec<(String, String)> {
    let mut seen: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut scanned = 0usize;
    let stories = std::iter::once(StoryRef::Body).chain(doc.parts.keys().map(|id| StoryRef::Part(*id)));
    'stories: for story in stories {
        for path in doc.para_paths(story) {
            scanned += 1;
            if scanned > MAX_SCANNED_PARAS {
                break 'stories;
            }
            let Some(p) = doc.para(story, &path) else { continue };
            for (r, c) in p.run_ranges() {
                if r.is_empty() {
                    continue;
                }
                let font = doc.styles.resolve_char(p.props.style.as_deref(), c).font;
                if let std::collections::btree_map::Entry::Vacant(e) = seen.entry(font) {
                    let shown = shown_as(e.key());
                    e.insert(shown);
                }
            }
        }
    }
    seen.into_iter().filter_map(|(font, shown)| Some((font, shown?))).collect()
}

fn missing(s: &mut Session, _: &Value) -> CmdResult {
    let fonts = missing_fonts(&s.doc);
    Ok(json!({"missing": fonts.iter().map(|(font, shown)| json!({"font": font, "shownAs": shown})).collect::<Vec<_>>()}))
}
