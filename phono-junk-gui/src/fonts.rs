//! Pan-script font configuration.
//!
//! Unlike retro-junk-gui's feature-gated CJK, phono-junk ships the full
//! multi-script Noto bundle as the default, baseline font layer. Foreign
//! discs (Korean, Chinese, Japanese, Hebrew) are the whole point —
//! requiring a rebuild to see them is wrong.
//!
//! Thai / Arabic / Devanagari are listed as future additions in CLAUDE.md
//! and can be added when Noto Sans Thai/Arabic/Devanagari TTFs land in
//! `fonts/`; they plug in alongside the existing bundle with no code
//! structure change.
//!
//! **CJK regional routing.** Han unification packs CJK codepoints shared
//! across Japanese, Simplified Chinese, Traditional Chinese, and Korean
//! into the same codepoints — but the canonical glyph forms differ per
//! region. egui's per-family fallback picks the first font that has the
//! glyph, which means "whichever CJK font we list first wins for all CJK
//! text", regardless of the actual language. To fix that we register each
//! CJK font under a named [`egui::FontFamily::Name`] so callers can pick
//! the right variant for language-tagged text via
//! [`family_for`] + `RichText::new(s).family(...)`.
//!
//! Font source: <https://github.com/notofonts/noto-cjk> (Sans2.004) for
//! CJK and <https://github.com/notofonts> for the rest. Licensed under
//! SIL OFL 1.1 (see `fonts/LICENSE`).

use egui::{FontData, FontDefinitions, FontFamily};

const NOTO_SANS_REGULAR: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
const NOTO_SANS_BOLD: &[u8] = include_bytes!("../fonts/NotoSans-Bold.ttf");
const NOTO_SANS_HEBREW: &[u8] = include_bytes!("../fonts/NotoSansHebrew-Regular.ttf");
const NOTO_SANS_JP: &[u8] = include_bytes!("../fonts/NotoSansJP-Regular.ttf");
const NOTO_SANS_KR: &[u8] = include_bytes!("../fonts/NotoSansKR-Regular.ttf");
const NOTO_SANS_SC: &[u8] = include_bytes!("../fonts/NotoSansSC-Regular.ttf");
const NOTO_SANS_TC: &[u8] = include_bytes!("../fonts/NotoSansTC-Regular.ttf");
const NOTO_SANS_HK: &[u8] = include_bytes!("../fonts/NotoSansHK-Regular.ttf");

// egui font names (arbitrary string keys, referenced by FontFamily membership).
const F_LATIN: &str = "noto_sans";
const F_BOLD: &str = "noto_sans_bold";
const F_HEBREW: &str = "noto_sans_hebrew";
const F_JP: &str = "noto_sans_jp";
const F_KR: &str = "noto_sans_kr";
const F_SC: &str = "noto_sans_sc";
const F_TC: &str = "noto_sans_tc";
const F_HK: &str = "noto_sans_hk";

// Named-family keys. Consumers route text through these by calling
// [`family_for`] and using `RichText::family(...)`.
pub const FAMILY_BOLD: &str = "bold";
pub const FAMILY_CJK_JP: &str = "cjk_jp";
pub const FAMILY_CJK_KR: &str = "cjk_kr";
pub const FAMILY_CJK_SC: &str = "cjk_sc";
pub const FAMILY_CJK_TC: &str = "cjk_tc";
pub const FAMILY_CJK_HK: &str = "cjk_hk";

/// Install the pan-script Noto bundle into egui's font system.
///
/// Loads every bundled font once, sets up the default [`FontFamily::Proportional`]
/// with Latin primary + CJK/Hebrew fallbacks (JP-first for Han-unified
/// glyphs when language is unknown), and registers named families for
/// region-aware overrides (`cjk_jp`, `cjk_kr`, `cjk_sc`, `cjk_tc`,
/// `cjk_hk`) plus a `bold` family for emphasized UI text.
pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // Register every font exactly once.
    fonts
        .font_data
        .insert(F_LATIN.into(), FontData::from_static(NOTO_SANS_REGULAR).into());
    fonts
        .font_data
        .insert(F_BOLD.into(), FontData::from_static(NOTO_SANS_BOLD).into());
    fonts
        .font_data
        .insert(F_HEBREW.into(), FontData::from_static(NOTO_SANS_HEBREW).into());
    fonts
        .font_data
        .insert(F_JP.into(), FontData::from_static(NOTO_SANS_JP).into());
    fonts
        .font_data
        .insert(F_KR.into(), FontData::from_static(NOTO_SANS_KR).into());
    fonts
        .font_data
        .insert(F_SC.into(), FontData::from_static(NOTO_SANS_SC).into());
    fonts
        .font_data
        .insert(F_TC.into(), FontData::from_static(NOTO_SANS_TC).into());
    fonts
        .font_data
        .insert(F_HK.into(), FontData::from_static(NOTO_SANS_HK).into());

    // Proportional family: Latin primary, then fallbacks. JP leads the CJK
    // fallbacks so untagged Han text renders with Japanese glyph forms
    // (the primary user's catalog bias). Tagged text overrides this via
    // [`family_for`] + `RichText::family`.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, F_LATIN.into());
    proportional.push(F_JP.into());
    proportional.push(F_SC.into());
    proportional.push(F_TC.into());
    proportional.push(F_HK.into());
    proportional.push(F_KR.into());
    proportional.push(F_HEBREW.into());

    // Monospace: keep egui's built-in mono primary. Add script fallbacks
    // so CJK/Hebrew in mono contexts (table cells with identifiers that
    // include foreign text, etc.) still render.
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.push(F_JP.into());
    mono.push(F_SC.into());
    mono.push(F_TC.into());
    mono.push(F_HK.into());
    mono.push(F_KR.into());
    mono.push(F_HEBREW.into());

    // Named families for explicit routing. Each CJK family uses its
    // region's font as primary, Latin as the Latin-glyph fallback, and
    // the other CJK fonts behind it so missing codepoints still resolve.
    install_named(&mut fonts, FAMILY_BOLD, &[F_BOLD, F_LATIN, F_JP, F_KR, F_SC, F_TC, F_HK, F_HEBREW]);
    install_named(&mut fonts, FAMILY_CJK_JP, &[F_JP, F_LATIN, F_SC, F_TC, F_HK, F_KR, F_HEBREW]);
    install_named(&mut fonts, FAMILY_CJK_KR, &[F_KR, F_LATIN, F_JP, F_SC, F_TC, F_HK, F_HEBREW]);
    install_named(&mut fonts, FAMILY_CJK_SC, &[F_SC, F_LATIN, F_TC, F_HK, F_JP, F_KR, F_HEBREW]);
    install_named(&mut fonts, FAMILY_CJK_TC, &[F_TC, F_LATIN, F_HK, F_SC, F_JP, F_KR, F_HEBREW]);
    install_named(&mut fonts, FAMILY_CJK_HK, &[F_HK, F_LATIN, F_TC, F_SC, F_JP, F_KR, F_HEBREW]);

    ctx.set_fonts(fonts);
}

fn install_named(fonts: &mut FontDefinitions, name: &str, stack: &[&str]) {
    fonts.families.insert(
        FontFamily::Name(name.into()),
        stack.iter().map(|s| (*s).to_string()).collect(),
    );
}

/// Pick the CJK `FontFamily` that matches the given language / script /
/// country metadata, or [`FontFamily::Proportional`] when there's no
/// regional signal.
///
/// Priority:
/// 1. Language codes (ISO 639-3) — `jpn`→JP, `kor`→KR, `zho`/`chi`
///    combined with script or country to disambiguate Simplified /
///    Traditional / Hong Kong.
/// 2. Script codes (ISO 15924) — `Jpan`/`Hrkt`/`Hira`/`Kana`→JP,
///    `Hang`/`Kore`→KR, `Hans`→SC, `Hant`→TC (or HK when country=HK).
/// 3. Country code (ISO 3166-1 alpha-2) as a last-resort proxy —
///    `JP`→JP, `KR`→KR, `CN`→SC, `TW`→TC, `HK`→HK.
///
/// Returns [`FontFamily::Proportional`] if no signal matches so untagged
/// text uses the default fallback chain.
pub fn family_for(
    language: Option<&str>,
    script: Option<&str>,
    country: Option<&str>,
) -> FontFamily {
    if let Some(fam) = from_language(language, script, country) {
        return fam;
    }
    if let Some(fam) = from_script(script, country) {
        return fam;
    }
    if let Some(fam) = from_country(country) {
        return fam;
    }
    FontFamily::Proportional
}

fn from_language(
    language: Option<&str>,
    script: Option<&str>,
    country: Option<&str>,
) -> Option<FontFamily> {
    let lang = language?.to_ascii_lowercase();
    match lang.as_str() {
        "jpn" => Some(named(FAMILY_CJK_JP)),
        "kor" => Some(named(FAMILY_CJK_KR)),
        "zho" | "chi" | "yue" | "cmn" => Some(zh_family(script, country)),
        _ => None,
    }
}

fn from_script(script: Option<&str>, country: Option<&str>) -> Option<FontFamily> {
    let s = script?;
    match s {
        "Jpan" | "Hrkt" | "Hira" | "Kana" => Some(named(FAMILY_CJK_JP)),
        "Hang" | "Kore" => Some(named(FAMILY_CJK_KR)),
        "Hans" => Some(named(FAMILY_CJK_SC)),
        "Hant" => Some(if country.map(|c| c.eq_ignore_ascii_case("HK")).unwrap_or(false) {
            named(FAMILY_CJK_HK)
        } else {
            named(FAMILY_CJK_TC)
        }),
        _ => None,
    }
}

fn from_country(country: Option<&str>) -> Option<FontFamily> {
    let c = country?;
    match c.to_ascii_uppercase().as_str() {
        "JP" => Some(named(FAMILY_CJK_JP)),
        "KR" => Some(named(FAMILY_CJK_KR)),
        "CN" => Some(named(FAMILY_CJK_SC)),
        "TW" => Some(named(FAMILY_CJK_TC)),
        "HK" => Some(named(FAMILY_CJK_HK)),
        _ => None,
    }
}

/// Resolve a Chinese-language tag to a family. `Hans` → SC; `Hant` with
/// country `HK` → HK, otherwise TC. With no script, country is the
/// tiebreaker (`CN`→SC, `TW`→TC, `HK`→HK); default to SC so at least
/// the text has a regional bias rather than falling back to JP's Han
/// glyph forms.
fn zh_family(script: Option<&str>, country: Option<&str>) -> FontFamily {
    match script {
        Some("Hans") => named(FAMILY_CJK_SC),
        Some("Hant") => {
            if country.map(|c| c.eq_ignore_ascii_case("HK")).unwrap_or(false) {
                named(FAMILY_CJK_HK)
            } else {
                named(FAMILY_CJK_TC)
            }
        }
        _ => match country.map(|c| c.to_ascii_uppercase()) {
            Some(ref c) if c == "HK" => named(FAMILY_CJK_HK),
            Some(ref c) if c == "TW" => named(FAMILY_CJK_TC),
            _ => named(FAMILY_CJK_SC),
        },
    }
}

fn named(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(f: &FontFamily) -> String {
        match f {
            FontFamily::Name(n) => n.to_string(),
            FontFamily::Proportional => "<Proportional>".into(),
            FontFamily::Monospace => "<Monospace>".into(),
        }
    }

    #[test]
    fn language_tag_wins_over_country() {
        // Japanese album pressed in the US — language beats country.
        let f = family_for(Some("jpn"), None, Some("US"));
        assert_eq!(name(&f), FAMILY_CJK_JP);
    }

    #[test]
    fn chinese_simplified_via_script() {
        let f = family_for(Some("zho"), Some("Hans"), None);
        assert_eq!(name(&f), FAMILY_CJK_SC);
    }

    #[test]
    fn chinese_traditional_hk_via_country() {
        let f = family_for(Some("zho"), Some("Hant"), Some("HK"));
        assert_eq!(name(&f), FAMILY_CJK_HK);
    }

    #[test]
    fn chinese_traditional_tw_default() {
        let f = family_for(Some("zho"), Some("Hant"), Some("TW"));
        assert_eq!(name(&f), FAMILY_CJK_TC);
    }

    #[test]
    fn script_only_jpn() {
        let f = family_for(None, Some("Jpan"), None);
        assert_eq!(name(&f), FAMILY_CJK_JP);
    }

    #[test]
    fn country_only_kr() {
        let f = family_for(None, None, Some("KR"));
        assert_eq!(name(&f), FAMILY_CJK_KR);
    }

    #[test]
    fn unknown_everything_falls_back_to_proportional() {
        let f = family_for(Some("eng"), Some("Latn"), Some("US"));
        assert_eq!(name(&f), "<Proportional>");
    }

    #[test]
    fn none_falls_back_to_proportional() {
        let f = family_for(None, None, None);
        assert_eq!(name(&f), "<Proportional>");
    }
}
