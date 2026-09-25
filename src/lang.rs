//! 言語ごとの検索の調整。英語・アメリカ中心の結果にならないようにする。
//!
//! 1. 言語コード(約130言語、`languages.rs`)を、検索元が使える言語・地域の指定(`hl`/`gl`)にそろえる。
//! 2. 検索元の結果のうち、その言語の文字で書かれたページを上に、関係のない言語のページを下に並べ直す
//!    (日本語ならかな・漢字、中国語なら漢字、韓国語ならハングル、アラビア語・ペルシア語ならアラビア文字 など)。
//!    検索元が英語のページを混ぜてきても、探している言語のページが先に来る。
//! 3. その言語を得意とする検索元(`EngineDef::langs`)を重くする。

use crate::languages;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// ラテン文字など、文字では見分けない(並べ直しをしない)
    Latin,
    Japanese,
    Chinese,
    Korean,
    Arabic,
    Cyrillic,
    Devanagari,
    Bengali,
    Thai,
    Hebrew,
    Greek,
    Other(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lang {
    /// `languages::LANGUAGES` のコード(例: zh-Hans)
    pub code: &'static str,
    /// 検索元へ渡す言語(例: zh-CN)
    pub hl: String,
    /// 検索元へ渡す地域(例: CN)。分からなければ空
    pub gl: String,
    pub script: Script,
}

fn script_of_char(c: char) -> Option<&'static str> {
    let u = c as u32;
    Some(match u {
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => "kana",
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => "han",
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => "hangul",
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => {
            "arabic"
        }
        0x0400..=0x052F => "cyrillic",
        0x0900..=0x097F => "devanagari",
        0x0980..=0x09FF => "bengali",
        0x0E00..=0x0E7F => "thai",
        0x0590..=0x05FF => "hebrew",
        0x0370..=0x03FF | 0x1F00..=0x1FFF => "greek",
        0x0A00..=0x0A7F => "gurmukhi",
        0x0A80..=0x0AFF => "gujarati",
        0x0B80..=0x0BFF => "tamil",
        0x0C00..=0x0C7F => "telugu",
        0x0C80..=0x0CFF => "kannada",
        0x0D00..=0x0D7F => "malayalam",
        0x0D80..=0x0DFF => "sinhala",
        0x0E80..=0x0EFF => "lao",
        0x1000..=0x109F => "myanmar",
        0x1780..=0x17FF => "khmer",
        0x10A0..=0x10FF => "georgian",
        0x0530..=0x058F => "armenian",
        0x1200..=0x137F => "ethiopic",
        0x0F00..=0x0FFF => "tibetan",
        _ if c.is_alphabetic() => "latin",
        _ => return None,
    })
}

/// 言語コード(基本部分)→ 使う文字の種類
fn script_for(base: &str) -> Script {
    match base {
        "ja" => Script::Japanese,
        "zh" | "yue" | "wuu" => Script::Chinese,
        "ko" => Script::Korean,
        "ar" | "fa" | "ur" | "ps" | "ckb" | "ug" | "sd" | "dv" => Script::Arabic,
        "ru" | "uk" | "bg" | "sr" | "mk" | "be" | "kk" | "ky" | "tg" | "mn" | "tt" | "ba"
        | "cv" => Script::Cyrillic,
        "hi" | "mr" | "ne" | "sa" | "kok" | "mai" | "bho" => Script::Devanagari,
        "bn" | "as" => Script::Bengali,
        "th" => Script::Thai,
        "he" | "yi" => Script::Hebrew,
        "el" => Script::Greek,
        "pa" => Script::Other("gurmukhi"),
        "gu" => Script::Other("gujarati"),
        "ta" => Script::Other("tamil"),
        "te" => Script::Other("telugu"),
        "kn" => Script::Other("kannada"),
        "ml" => Script::Other("malayalam"),
        "si" => Script::Other("sinhala"),
        "lo" => Script::Other("lao"),
        "my" => Script::Other("myanmar"),
        "km" => Script::Other("khmer"),
        "ka" => Script::Other("georgian"),
        "hy" => Script::Other("armenian"),
        "am" | "ti" => Script::Other("ethiopic"),
        "bo" | "dz" => Script::Other("tibetan"),
        _ => Script::Latin,
    }
}

/// 言語の既定の地域(検索元の `gl` / `cc`)
fn default_region(code: &str) -> &'static str {
    match code {
        "ja" => "JP",
        "en" => "US",
        "zh-Hans" => "CN",
        "zh-Hant" => "TW",
        "ko" => "KR",
        "es" => "ES",
        "pt-BR" => "BR",
        "pt-PT" => "PT",
        "fr" => "FR",
        "de" => "DE",
        "it" => "IT",
        "ru" => "RU",
        "uk" => "UA",
        "ar" => "SA",
        "he" => "IL",
        "fa" => "IR",
        "tr" => "TR",
        "hi" => "IN",
        "bn" => "BD",
        "ur" => "PK",
        "pa" | "gu" | "mr" | "ta" | "te" | "kn" | "ml" | "or" | "as" => "IN",
        "ne" => "NP",
        "si" => "LK",
        "th" => "TH",
        "lo" => "LA",
        "km" => "KH",
        "my" => "MM",
        "vi" => "VN",
        "id" => "ID",
        "ms" => "MY",
        "fil" | "tl" => "PH",
        "pl" => "PL",
        "nl" => "NL",
        "sv" => "SE",
        "da" => "DK",
        "no" | "nb" | "nn" => "NO",
        "fi" => "FI",
        "cs" => "CZ",
        "sk" => "SK",
        "hu" => "HU",
        "ro" => "RO",
        "bg" => "BG",
        "el" => "GR",
        "sr" => "RS",
        "hr" => "HR",
        "sl" => "SI",
        "lt" => "LT",
        "lv" => "LV",
        "et" => "EE",
        "ka" => "GE",
        "hy" => "AM",
        "az" => "AZ",
        "kk" => "KZ",
        "uz" => "UZ",
        "mn" => "MN",
        "sw" => "KE",
        "am" => "ET",
        "af" => "ZA",
        "ha" | "yo" | "ig" => "NG",
        "is" => "IS",
        "ga" => "IE",
        "ca" => "ES",
        _ => "",
    }
}

/// 呼び出し側が渡した `hl`(例: ja、zh-TW、zh-Hant、pt_BR)と `gl` を、検索に使う形にそろえる。
/// 一覧に無い言語でも、コードの形が正しければそのまま検索元へ渡す(並べ直しはしない)。
pub fn resolve(hl: &str, gl: &str) -> Lang {
    let raw = hl.trim().replace('_', "-");
    let lower = raw.to_ascii_lowercase();
    let code: Option<&'static str> = match lower.as_str() {
        "zh" | "zh-cn" | "zh-sg" | "zh-hans" => Some("zh-Hans"),
        "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant" => Some("zh-Hant"),
        "pt" | "pt-br" => Some("pt-BR"),
        "pt-pt" => Some("pt-PT"),
        "nb" | "nn" => Some("no"),
        "tl" => Some("fil"),
        "iw" => Some("he"),
        _ => languages::find(&lower)
            .or_else(|| languages::find(lower.split('-').next().unwrap_or("")))
            .map(|l| l.0),
    };
    let base = lower.split('-').next().unwrap_or("en");
    let known = code.is_some();
    let (code, script) = match code {
        Some(c) => (c, script_for(c.split('-').next().unwrap_or(c))),
        None => ("en", script_for(base)),
    };
    let engine_hl = match code {
        "zh-Hans" => "zh-CN".to_string(),
        "zh-Hant" => "zh-TW".to_string(),
        c if c.contains('-') => c.to_string(),
        c => c.to_string(),
    };
    let gl = gl.trim().to_ascii_uppercase();
    let gl = if gl.is_empty() {
        default_region(code).to_string()
    } else {
        gl
    };
    Lang {
        code,
        hl: if lower.is_empty() {
            "en".into()
        } else if known {
            engine_hl
        } else {
            lower.clone() // 一覧に無い言語は、そのまま検索元へ渡す
        },
        gl,
        script,
    }
}

fn wanted(script: Script) -> &'static [&'static str] {
    match script {
        Script::Latin => &[],
        Script::Japanese => &["kana", "han"],
        Script::Chinese => &["han"],
        Script::Korean => &["hangul", "han"],
        Script::Arabic => &["arabic"],
        Script::Cyrillic => &["cyrillic"],
        Script::Devanagari => &["devanagari"],
        Script::Bengali => &["bengali"],
        Script::Thai => &["thai"],
        Script::Hebrew => &["hebrew"],
        Script::Greek => &["greek"],
        Script::Other(s) => match s {
            "gurmukhi" => &["gurmukhi"],
            "gujarati" => &["gujarati"],
            "tamil" => &["tamil"],
            "telugu" => &["telugu"],
            "kannada" => &["kannada"],
            "malayalam" => &["malayalam"],
            "sinhala" => &["sinhala"],
            "lao" => &["lao"],
            "myanmar" => &["myanmar"],
            "khmer" => &["khmer"],
            "georgian" => &["georgian"],
            "armenian" => &["armenian"],
            "ethiopic" => &["ethiopic"],
            "tibetan" => &["tibetan"],
            _ => &[],
        },
    }
}

/// 結果の文章が、探している言語の文字で書かれている度合い(0.0〜1.0)。文字が無ければ None。
pub fn script_share(script: Script, text: &str) -> Option<f64> {
    let want = wanted(script);
    if want.is_empty() {
        return None;
    }
    let (mut total, mut hit) = (0usize, 0usize);
    let mut has_kana = false;
    for c in text.chars() {
        let Some(s) = script_of_char(c) else { continue };
        total += 1;
        if s == "kana" {
            has_kana = true;
        }
        if want.contains(&s) {
            hit += 1;
        }
    }
    if total == 0 {
        return None;
    }
    // 中国語を探していて、かなが混じる文章は日本語のページ
    if script == Script::Chinese && has_kana {
        return Some(0.0);
    }
    Some(hit as f64 / total as f64)
}

/// 探している言語の文字で書かれたページを上に、そうでないページを下にするための倍率。
pub fn rank_factor(script: Script, title: &str, snippet: &str) -> f64 {
    let text = format!("{title} {snippet}");
    match script_share(script, &text) {
        None => 1.0,
        Some(s) if s >= 0.3 => 1.5,
        Some(s) if s > 0.0 => 1.0,
        Some(_) => 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_languages_and_regions() {
        let l = resolve("ja", "");
        assert_eq!(
            (l.code, l.hl.as_str(), l.gl.as_str(), l.script),
            ("ja", "ja", "JP", Script::Japanese)
        );
        let l = resolve("zh-TW", "");
        assert_eq!(
            (l.code, l.hl.as_str(), l.gl.as_str()),
            ("zh-Hant", "zh-TW", "TW")
        );
        let l = resolve("zh", "");
        assert_eq!(
            (l.code, l.hl.as_str(), l.gl.as_str()),
            ("zh-Hans", "zh-CN", "CN")
        );
        let l = resolve("fa", "");
        assert_eq!((l.gl.as_str(), l.script), ("IR", Script::Arabic));
        let l = resolve("ar", "EG");
        assert_eq!(l.gl, "EG", "地域は指定を優先");
        let l = resolve("pt_BR", "");
        assert_eq!((l.code, l.gl.as_str()), ("pt-BR", "BR"));
        let l = resolve("", "");
        assert_eq!((l.code, l.hl.as_str()), ("en", "en"));
    }

    #[test]
    fn every_listed_language_resolves_to_itself() {
        for (code, _, _) in languages::LANGUAGES {
            let l = resolve(code, "");
            assert_eq!(l.code, *code, "{code}");
        }
    }

    #[test]
    fn japanese_pages_outrank_english_ones_for_japanese_queries() {
        let ja = rank_factor(Script::Japanese, "山梨県の温泉一覧", "日帰り温泉・旅館");
        let en = rank_factor(
            Script::Japanese,
            "Hot springs in Yamanashi",
            "A list of onsen",
        );
        assert!(ja > en, "{ja} {en}");
        assert_eq!(rank_factor(Script::Latin, "anything", ""), 1.0);
    }

    #[test]
    fn chinese_korean_arabic_persian_are_recognised() {
        assert!(rank_factor(Script::Chinese, "北京温泉推荐", "") > 1.0);
        assert!(
            rank_factor(Script::Chinese, "山梨県の温泉です", "かなが混じる") < 1.0,
            "日本語のページは中国語の結果にしない"
        );
        assert!(rank_factor(Script::Korean, "서울 맛집 추천", "") > 1.0);
        assert!(rank_factor(Script::Arabic, "أفضل المطاعم في دبي", "") > 1.0);
        assert!(rank_factor(Script::Arabic, "بهترین رستوران‌های تهران", "") > 1.0);
        assert!(rank_factor(Script::Cyrillic, "Лучшие рестораны Москвы", "") > 1.0);
        assert!(rank_factor(Script::Thai, "ร้านอาหารกรุงเทพ", "") > 1.0);
        assert!(rank_factor(Script::Devanagari, "दिल्ली के रेस्तरां", "") > 1.0);
        assert!(rank_factor(Script::Korean, "Seoul restaurants", "") < 1.0);
    }
}
