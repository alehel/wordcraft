//! Fonts added at run time (#582): `font.add`, `font.list`, `font.remove`.

use serde_json::json;
use wordcraft_doc::Pos;

use crate::Session;
use crate::cmd::insert::base64_encode;

/// The bundled face `file` of family `original` renamed `family` (as long as `original`), so it
/// is a font the database doesn't have.
fn renamed(file: &str, original: &str, family: &str) -> Vec<u8> {
    assert_eq!(family.len(), original.len());
    let mut data = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts").join(file)).unwrap();
    let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>();
    for (from, to) in [(utf16(original), utf16(family)), (original.as_bytes().to_vec(), family.as_bytes().to_vec())] {
        let mut i = 0;
        while let Some(at) = data[i..].windows(from.len()).position(|w| w == from) {
            data[i + at..i + at + to.len()].copy_from_slice(&to);
            i += at + to.len();
        }
    }
    data
}

/// The caret's x after `text` set in `font`.
fn text_end_x(s: &mut Session, font: &str, text: &str) -> f32 {
    s.run("document.setText", &json!({"text": text})).unwrap();
    s.run("select.all", &json!({})).unwrap();
    s.run("format.font", &json!({"name": font})).unwrap();
    s.layout().caret_on(&Pos::body(0, text.len()), 0).unwrap().x
}

#[test]
fn an_added_font_is_listed_and_lays_out_text_already_set_in_it() {
    // A monospaced face, so it measures unlike whatever proportional font the system falls back
    // to (Source Serif 4 itself where no system fonts are installed, as on the FreeBSD runner).
    // The name has no Serif/Sans/Mono in it, which would pick a fallback of that kind.
    const FAMILY: &str = "Cmdfont Face 4";
    // Narrow letters: set monospaced they take far more room than in any proportional font.
    const TEXT: &str = "illicit little lilies fill tiny jill";
    let mut s = Session::new(wordcraft_doc::Document::new());
    let mono = text_end_x(&mut s, "JetBrains Mono", TEXT);
    // Not installed yet: the text falls back to another font.
    let missing = text_end_x(&mut s, FAMILY, TEXT);
    assert!((missing - mono).abs() > 1.0, "the fallback measures differently: {missing} vs {mono}");

    let data = base64_encode(&renamed("JetBrainsMono-Regular.ttf", "JetBrains Mono", FAMILY));
    let r = s.run("font.add", &json!({"data": data, "name": "CmdfontFace4-Regular.ttf"})).unwrap();
    assert_eq!(r["families"], json!([FAMILY]));
    assert_eq!(r["faces"][0]["style"], "Regular");
    // The text already set in it re-lays out with it, without touching the document.
    let x = s.layout().caret_on(&Pos::body(0, TEXT.len()), 0).unwrap().x;
    assert!((x - mono).abs() < 0.01, "measured with the added font: {x} vs {mono} (missing {missing})");

    let list = s.run("font.list", &json!({"all": true})).unwrap();
    assert!(list["fonts"].as_array().unwrap().iter().any(|f| f["families"] == json!([FAMILY])));
    assert!(list["families"].as_array().unwrap().iter().any(|f| f == FAMILY));
    // It exports to PDF like any other font.
    assert!(crate::io::save_bytes("out.pdf", &s.doc).unwrap().starts_with(b"%PDF"));

    // Adding it again changes nothing.
    let again = s.run("font.add", &json!({"data": data, "name": "again.ttf"}));
    assert!(again.unwrap_err().to_string().contains("already available"));

    let removed = s.run("font.remove", &json!({"family": FAMILY})).unwrap();
    assert_eq!(removed["removed"].as_array().unwrap().len(), 1);
    let x = s.layout().caret_on(&Pos::body(0, TEXT.len()), 0).unwrap().x;
    assert!((x - missing).abs() < 0.01, "falls back again once removed");
    assert!(s.run("font.remove", &json!({"family": FAMILY})).is_err());
}

#[test]
fn adding_a_font_never_changes_text_that_does_not_use_it() {
    const FAMILY: &str = "Metric Serif 4";
    let mut s = Session::new(crate::sample::sample_document());
    let before = s.layout();
    let positions = |s: &mut Session| {
        let l = s.layout();
        (0..s.doc.body.len().min(12)).filter_map(|b| l.caret_on(&Pos::body(b, 0), 0).map(|c| (c.page, c.x, c.top))).collect::<Vec<_>>()
    };
    let a = positions(&mut s);
    s.run("font.add", &json!({"data": base64_encode(&renamed("SourceSerif4-Bold.ttf", "Source Serif 4", FAMILY)), "name": "m.ttf"})).unwrap();
    assert_eq!(positions(&mut s), a);
    assert_eq!(s.layout().pages.len(), before.pages.len());
}

#[test]
fn bad_font_input_is_an_error() {
    let mut s = Session::new(wordcraft_doc::Document::new());
    for p in [
        json!({"data": base64_encode(b"definitely not a font")}),
        json!({"data": "%%% not base64"}),
        json!({"data": base64_encode(b"wOF2\0\x01\0\0\xff\xff\xff\xff")}),
        json!({"path": "/nonexistent/font.ttf"}),
        json!({"path": env!("CARGO_MANIFEST_DIR")}),
        json!({"path": concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")}),
        json!({"url": "https://fonts.googleapis.com/css2?family=Roboto"}),
    ] {
        assert!(s.run("font.add", &p).is_err(), "{p}");
    }
    assert!(s.run("font.remove", &json!({})).is_err());
    assert!(s.run("font.remove", &json!({"family": "Source Sans 3"})).is_err(), "the bundled fonts stay");
    // No file: the UI is asked for a picker.
    let r = s.run("font.add", &json!({})).unwrap();
    assert_eq!(r["pending"], true);
    assert!(s.ui_requests.iter().any(|r| r["open"] == "addFont"));
}

/// `font.missing`'s fonts, as "font>shownAs".
fn missing_list(s: &mut Session) -> Vec<String> {
    let m = s.run("font.missing", &json!({})).unwrap();
    m["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let shown: Vec<&str> = f["shownAs"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
            format!("{}>{}", f["font"].as_str().unwrap(), shown.join(","))
        })
        .collect()
}

/// The missing fonts of `doc` as laid out.
fn missing_in(doc: wordcraft_doc::Document) -> Vec<crate::cmd::font::MissingFont> {
    crate::cmd::font::missing_fonts(&Session::new(doc).layout())
}

/// A paragraph of `text` set in `font`.
fn para_in(text: &str, font: &str) -> wordcraft_doc::Paragraph {
    wordcraft_doc::Paragraph::with_text(text, wordcraft_doc::CharProps { font: Some(font.into()), ..Default::default() })
}

/// A document of these paragraphs.
fn doc_of(paras: Vec<wordcraft_doc::Paragraph>) -> wordcraft_doc::Document {
    let mut doc = wordcraft_doc::Document::new();
    doc.body = paras.into_iter().map(wordcraft_doc::para_block).collect();
    doc
}

#[test]
fn fonts_that_are_not_installed_are_reported_with_the_font_shown_instead() {
    // A family no system has, with no Serif/Mono in its name: it's drawn in a sans substitute.
    const MISSING: &str = "Wordcraft Nowhere Face";
    let mut s = Session::new(wordcraft_doc::Document::new());
    s.run("document.setText", &json!({"text": "Bundled\nMissing"})).unwrap();
    s.run("select.all", &json!({})).unwrap();
    s.run("format.font", &json!({"name": "JetBrains Mono"})).unwrap();
    assert!(s.run("format.state", &json!({})).unwrap()["fontShownAs"].is_null(), "a bundled font is installed");
    assert_eq!(missing_list(&mut s), Vec::<String>::new());

    s.run("select.text", &json!({"text": "Missing"})).unwrap();
    s.run("format.font", &json!({"name": MISSING})).unwrap();
    let st = s.run("format.state", &json!({})).unwrap();
    assert_eq!(st["font"], MISSING);
    let shown = st["fontShownAs"].as_str().unwrap_or_default().to_string();
    assert!(!shown.is_empty() && shown != MISSING, "the font box learns the substitute: {st}");
    assert_eq!(missing_list(&mut s), [format!("{MISSING}>{shown}")], "only the missing font, once");

    // A mixed selection has no single font, so nothing to flag in the font box.
    s.run("select.all", &json!({})).unwrap();
    assert!(s.run("format.state", &json!({})).unwrap()["fontShownAs"].is_null());
}

#[test]
fn a_font_only_a_style_names_is_not_reported_missing() {
    let mut s = Session::new(wordcraft_doc::Document::new());
    s.run("document.setText", &json!({"text": "Body"})).unwrap();
    s.run("styles.create", &json!({"name": "Unused Look"})).unwrap();
    s.run("styles.modify", &json!({"style": "Unused Look", "chr": {"font": "Wordcraft Style Only Face"}})).unwrap();
    // Creating a style applies it, as in Word: put the text back in Normal.
    s.run("para.style", &json!({"style": "Normal"})).unwrap();
    s.run("select.all", &json!({})).unwrap();
    s.run("format.font", &json!({"name": "JetBrains Mono"})).unwrap();
    assert_eq!(missing_list(&mut s), Vec::<String>::new(), "no text uses the style");
    // Text in the style, with no font of its own, is.
    let mut s = Session::new(wordcraft_doc::Document::new());
    s.run("document.setText", &json!({"text": "Body"})).unwrap();
    s.run("styles.create", &json!({"name": "Used Look"})).unwrap();
    s.run("styles.modify", &json!({"style": "Used Look", "chr": {"font": "Wordcraft Style Only Face"}})).unwrap();
    s.run("para.style", &json!({"style": "Used Look"})).unwrap();
    let list = missing_list(&mut s);
    assert!(list.iter().any(|f| f.starts_with("Wordcraft Style Only Face>")), "{list:?}");
}

#[test]
fn missing_fonts_are_what_layout_drew_in_shown_text() {
    let mut doc = doc_of(vec![
        para_in("one", "Wordcraft Nowhere Face"),
        para_in("two", "wordcraft nowhere face"),
        para_in("three", "JetBrains Mono"),
        // Spaces, a picture's placeholder and zero-width characters show no font.
        para_in(" \u{FFFC}\t\u{200B}\u{FEFF}", "Wordcraft Blank Face"),
    ]);
    // Hidden text isn't laid out.
    doc.body.push(wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text(
        "secret",
        wordcraft_doc::CharProps { font: Some("Wordcraft Hidden Face".into()), hidden: Some(true), ..Default::default() },
    )));
    // A text box story nothing shows any more (deleted, not yet pruned).
    doc.parts.insert(
        77,
        wordcraft_doc::Part {
            kind: wordcraft_doc::PartKind::TextBox,
            blocks: vec![wordcraft_doc::para_block(para_in("gone", "Wordcraft Orphan Face"))],
            body: Default::default(),
        },
    );
    let names: Vec<String> = missing_in(doc).into_iter().map(|f| f.font).collect();
    assert_eq!(names.len(), 1, "one font whatever its case, and nothing else: {names:?}");
    assert!(names[0].eq_ignore_ascii_case("Wordcraft Nowhere Face"));
}

#[test]
fn metric_twins_are_listed_but_flagged_compatible() {
    // A twin by name, installed or not here: "Liberation Sans" stands in for Arial wherever it's
    // the substitute; a made-up family's substitute is never its twin.
    let m = missing_in(doc_of(vec![para_in("Text 😀", "Arial"), para_in("Text", "Wordcraft Nowhere Face")]));
    let nowhere = m.iter().find(|f| f.font == "Wordcraft Nowhere Face").expect("listed");
    assert!(!nowhere.metric_compatible, "{nowhere:?}");
    match crate::cmd::font::shown_as("Arial") {
        // Arial is installed: not listed.
        None => assert!(!m.iter().any(|f| f.font == "Arial"), "{m:?}"),
        Some(sub) => {
            let arial = m.iter().find(|f| f.font == "Arial").expect("listed");
            // An emoji drawn in a fallback face doesn't change the verdict on the substitute.
            assert_eq!(arial.metric_compatible, wordcraft_fonts::word::metric_compatible("Arial", &sub), "{arial:?}");
        }
    }
}

#[test]
fn a_missing_east_asian_font_is_reported_for_cjk_text_only() {
    let props =
        wordcraft_doc::CharProps { font: Some("JetBrains Mono".into()), font_ea: Some("Wordcraft Nowhere Mincho".into()), ..Default::default() };
    let m = missing_in(doc_of(vec![wordcraft_doc::Paragraph::with_text("abc 日本語", props.clone())]));
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].font, "Wordcraft Nowhere Mincho");
    assert!(!m[0].shown_as.is_empty() && !m[0].metric_compatible, "{m:?}");
    // Without CJK text the East Asian font draws nothing.
    assert!(missing_in(doc_of(vec![wordcraft_doc::Paragraph::with_text("abc", props)])).is_empty());
}

#[test]
fn the_font_box_warns_by_the_same_rule_as_the_status_bar() {
    let mut s = Session::new(wordcraft_doc::Document::new());
    s.run("document.setText", &json!({"text": "Text \u{F0FC}"})).unwrap();
    s.run("select.all", &json!({})).unwrap();
    let state_for = |s: &mut Session, font: &str| {
        s.run("format.font", &json!({"name": font})).unwrap();
        s.run("format.state", &json!({})).unwrap()
    };
    // A made-up family: drawn in a substitute that measures differently.
    let st = state_for(&mut s, "Wordcraft Nowhere Face");
    assert!(st["fontWarning"].is_string() && st["fontWarning"] == st["fontShownAs"], "{st}");
    // A symbol font is drawn as Unicode on purpose: shown as a substitute, but no warning, as
    // the status bar leaves it out too.
    let st = state_for(&mut s, "Wingdings");
    assert!(st["fontWarning"].is_null(), "{st}");
    assert!(crate::cmd::font::missing_fonts(&s.layout()).is_empty());
    // A metric twin, wherever Calibri isn't installed: the verdict matches the status bar's.
    let st = state_for(&mut s, "Calibri");
    let listed = crate::cmd::font::missing_fonts(&s.layout());
    let flagged = listed.iter().any(|f| f.font == "Calibri" && !f.metric_compatible);
    assert_eq!(st["fontWarning"].is_string(), flagged, "{st} vs {listed:?}");
}

#[test]
fn symbol_fonts_drawn_as_unicode_are_not_reported() {
    // Word's bullets are Symbol characters; WordCraft draws them as their Unicode equivalents.
    let m = missing_in(doc_of(vec![para_in("\u{F0B7}", "Symbol"), para_in("\u{F0FC}", "Wingdings")]));
    assert!(m.is_empty(), "{m:?}");
}

#[test]
fn cjk_text_is_reported_under_its_east_asian_font_not_the_latin_one() {
    let props =
        wordcraft_doc::CharProps { font: Some("Wordcraft Latin Gone".into()), font_ea: Some("Wordcraft Asian Gone".into()), ..Default::default() };
    let names = |text: &str| -> Vec<String> {
        missing_in(doc_of(vec![wordcraft_doc::Paragraph::with_text(text, props.clone())])).into_iter().map(|f| f.font).collect()
    };
    assert_eq!(names("日本語"), ["Wordcraft Asian Gone"], "no character asks for the Latin font");
    assert_eq!(names("abc 日本語"), ["Wordcraft Asian Gone", "Wordcraft Latin Gone"]);
    // Without an East Asian font the CJK text asks for the Latin one.
    let latin_only = wordcraft_doc::CharProps { font: Some("Wordcraft Latin Gone".into()), ..Default::default() };
    let m = missing_in(doc_of(vec![wordcraft_doc::Paragraph::with_text("日本語", latin_only)]));
    assert_eq!(m.iter().map(|f| f.font.as_str()).collect::<Vec<_>>(), ["Wordcraft Latin Gone"]);
}

#[test]
fn a_symbol_font_named_for_complex_script_text_is_not_reported() {
    let props = wordcraft_doc::CharProps { font: Some("JetBrains Mono".into()), font_cs: Some("Symbol".into()), ..Default::default() };
    assert!(missing_in(doc_of(vec![wordcraft_doc::Paragraph::with_text("سلام", props)])).is_empty());
}

#[test]
fn a_hostile_number_of_missing_fonts_stays_bounded() {
    let paras = (0..3000).map(|i| para_in("x", &format!("Wordcraft Hostile Face {i}"))).collect();
    let m = missing_in(doc_of(paras));
    assert!(!m.is_empty() && m.len() <= wordcraft_layout::MAX_SUBSTITUTED, "{}", m.len());
}
