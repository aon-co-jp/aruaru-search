//! 検索元(エンジン)の定義と、公開の検索結果ページ(HTML)の解析。
//!
//! 検索元ごとの違いは、コードではなく**設定(`EngineDef`)**にする。URL の型と、結果の
//! 「かたまり・タイトル・リンク・要約」を指す CSS セレクタだけである。検索元がページの作りを
//! 変えても、セレクタの設定を直せば追従できる(`maintain` が AI の提案を検証して自動で直す)。

use anyhow::{anyhow, bail, Result};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EngineDef {
    pub id: String,
    pub name: String,
    /// 検索 URL の型。`{q}`(検索語)・`{hl}`(言語)・`{gl}`(地域)・`{kl}`(DuckDuckGo 形式)を置き換える
    pub url: String,
    /// 1件の結果のかたまり
    pub container: String,
    pub title: String,
    pub link: String,
    pub snippet: String,
    /// リンクの URL を読む属性。空なら link 要素の `href`。`data-href` のように別の属性も、`@mu` のように
    /// 結果のかたまり(container)自身の属性も指定できる
    #[serde(default)]
    pub link_attr: String,
    /// 転送 URL(例: DuckDuckGo の `uddg`)から本来の URL を取り出すためのパラメータ名。無ければ空
    #[serde(default)]
    pub unwrap: String,
    /// 統合するときの重み
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// この検索元が得意な言語(基本コード。例: ["ja"])。空なら全言語向け
    #[serde(default)]
    pub langs: Vec<String>,
    /// true なら、`langs` の言語の検索でしか使わない(例: 日本向けの検索元)
    #[serde(default)]
    pub only_langs: bool,
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hit {
    pub title: String,
    pub link: String,
    pub snippet: String,
}

pub fn defaults() -> Vec<EngineDef> {
    serde_json::from_str(include_str!("../engines.default.json"))
        .expect("既定の検索元の設定は正しい")
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl EngineDef {
    /// 検索 URL を作る。`hl` は言語(例: ja)、`gl` は地域(例: JP)。
    pub fn search_url(&self, q: &str, hl: &str, gl: &str) -> String {
        let hl = if hl.is_empty() { "en" } else { hl };
        let base = hl.split('-').next().unwrap_or(hl);
        let kl = if gl.is_empty() {
            "wt-wt".to_string() // DuckDuckGo の「地域を指定しない」
        } else {
            format!("{}-{}", gl.to_ascii_lowercase(), base.to_ascii_lowercase())
        };
        let url = self
            .url
            .replace("{q}", &encode(q))
            .replace("{hl}", &encode(hl))
            .replace("{gl}", &encode(gl))
            .replace("{kl}", &encode(&kl));
        // 地域が分からないときは、地域の指定そのものを付けない
        url.replace("&cc=&", "&")
            .trim_end_matches("&cc=")
            .to_string()
    }

    /// この検索元を、この言語の検索に使うか。
    pub fn serves(&self, lang_code: &str) -> bool {
        let base = lang_code.split('-').next().unwrap_or(lang_code);
        !self.only_langs || self.langs.iter().any(|l| l == base || l == lang_code)
    }

    /// この言語を得意とする検索元か(結果の統合で重くする)
    pub fn favours(&self, lang_code: &str) -> bool {
        let base = lang_code.split('-').next().unwrap_or(lang_code);
        self.langs.iter().any(|l| l == base || l == lang_code)
    }

    /// 設定が安全で使えるか(AI が提案した設定を取り込む前にも使う)。
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            bail!("id が正しくありません");
        }
        let u = reqwest::Url::parse(&self.search_url("test", "en", "US"))
            .map_err(|e| anyhow!("URL の型が正しくありません: {e}"))?;
        if u.scheme() != "https" {
            bail!("URL は https のみです");
        }
        for (name, sel) in [
            ("container", &self.container),
            ("title", &self.title),
            ("link", &self.link),
            ("snippet", &self.snippet),
        ] {
            if sel.len() > 300 {
                bail!("{name} のセレクタが長すぎます");
            }
            if sel.is_empty() && name == "snippet" {
                continue; // 要約が無い検索元もある
            }
            Selector::parse(sel)
                .map_err(|e| anyhow!("{name} のセレクタが正しくありません: {e:?}"))?;
        }
        if self.link_attr.len() > 40
            || !self
                .link_attr
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '@'))
        {
            bail!("link_attr が正しくありません");
        }
        if !(0.0..=2.0).contains(&self.weight) {
            bail!("weight は 0〜2 です");
        }
        Ok(())
    }
}

/// `//duckduckgo.com/l/?uddg=<URL>` のような転送 URL から、本来の URL を取り出す。
/// Bing の転送 URL(`bing.com/ck/a?...&u=a1<base64url>`)から本来の URL を取り出す。
fn decode_bing(href: &str) -> Option<String> {
    let u = reqwest::Url::parse(href).ok()?;
    if !u.host_str()?.ends_with("bing.com") {
        return None;
    }
    let (_, v) = u.query_pairs().find(|(k, _)| k == "u")?;
    let b64 = v.strip_prefix("a1")?;
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut out = Vec::new();
    for c in b64.chars() {
        let v = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '-' | '+' => 62,
            '_' | '/' => 63,
            '=' => break,
            _ => return None,
        };
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((bits >> nbits) & 0xFF) as u8);
        }
    }
    let s = String::from_utf8(out).ok()?;
    (s.starts_with("http://") || s.starts_with("https://")).then_some(s)
}

fn unwrap_redirect(href: &str, param: &str) -> String {
    if let Some(real) = decode_bing(href) {
        return real;
    }
    if param.is_empty() {
        return href.to_string();
    }
    let full = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.to_string()
    };
    if let Ok(u) = reqwest::Url::parse(&full) {
        if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == param) {
            return v.into_owned();
        }
    }
    href.to_string()
}

fn text_of(e: scraper::ElementRef<'_>) -> String {
    e.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 結果ページの HTML から、検索結果を取り出す。広告(検索元の宣伝リンク)は除く。
pub fn parse(def: &EngineDef, html: &str) -> Vec<Hit> {
    let snip = Selector::parse(&def.snippet).ok();
    let (Ok(cont), Ok(title), Ok(link)) = (
        Selector::parse(&def.container),
        Selector::parse(&def.title),
        Selector::parse(&def.link),
    ) else {
        return Vec::new();
    };
    let doc = Html::parse_document(html);
    let mut out: Vec<Hit> = Vec::new();
    for c in doc.select(&cont) {
        let Some(t) = c
            .select(&title)
            .next()
            .map(text_of)
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let href = match def.link_attr.strip_prefix('@') {
            Some(name) => c.value().attr(name),
            None => {
                let attr = if def.link_attr.is_empty() {
                    "href"
                } else {
                    def.link_attr.as_str()
                };
                c.select(&link).next().and_then(|a| a.value().attr(attr))
            }
        };
        let Some(href) = href else {
            continue;
        };
        let url = unwrap_redirect(href, &def.unwrap);
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        // 検索元自身の広告・転送は結果にしない
        if url.contains("duckduckgo.com/y.js") || url.contains("bing.com/aclick") {
            continue;
        }
        let s = snip
            .as_ref()
            .and_then(|sel| c.select(sel).next())
            .map(text_of)
            .unwrap_or_default();
        if out.iter().any(|h| h.link == url) {
            continue;
        }
        out.push(Hit {
            title: t,
            link: url,
            snippet: s,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ddg() -> EngineDef {
        defaults()
            .into_iter()
            .find(|e| e.id == "duckduckgo")
            .unwrap()
    }

    #[test]
    fn defaults_are_valid_and_urls_are_encoded() {
        for e in defaults() {
            e.validate().unwrap_or_else(|err| panic!("{}: {err}", e.id));
        }
        let u = ddg().search_url("山梨県 温泉", "ja", "JP");
        assert!(
            u.contains("q=%E5%B1%B1%E6%A2%A8%E7%9C%8C+%E6%B8%A9%E6%B3%89")
                && u.contains("kl=jp-ja"),
            "{u}"
        );
    }

    #[test]
    fn parses_duckduckgo_style_results_and_skips_ads() {
        let html = r#"<html><body>
          <div class="result results_links web-result"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&amp;rut=x">Example A</a></h2><a class="result__snippet">About A</a></div>
          <div class="result result--ad"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fad.example%2F">AD</a></h2></div>
          <div class="result"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fduckduckgo.com%2Fy.js%3Fad_domain%3Dx">Sneaky ad</a></h2></div>
          <div class="result"><h2><a class="result__a" href="javascript:alert(1)">Bad</a></h2></div>
          <div class="result"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa">Dup</a></h2></div>
          <div class="result"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fb">Example B</a></h2></div>
        </body></html>"#;
        let hits = parse(&ddg(), html);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(
            hits[0],
            Hit {
                title: "Example A".into(),
                link: "https://example.com/a".into(),
                snippet: "About A".into()
            }
        );
        assert_eq!(hits[1].link, "https://example.org/b");
    }

    #[test]
    fn decodes_bing_redirect_links() {
        // https://example.com/a?b=1 を base64url にして a1 を付けたもの
        let href = "https://www.bing.com/ck/a?!&&p=x&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9hP2I9MQ&ntb=1";
        assert_eq!(unwrap_redirect(href, ""), "https://example.com/a?b=1");
        let other = "https://other.example/?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9hP2I9MQ";
        assert_eq!(unwrap_redirect(other, ""), other);
    }

    #[test]
    fn empty_region_omits_cc_and_language_gating_works() {
        let bing = defaults().into_iter().find(|e| e.id == "bing").unwrap();
        let u = bing.search_url("x", "th", "");
        assert!(!u.contains("cc="), "{u}");
        let y = defaults().into_iter().find(|e| e.id == "yahoo-jp").unwrap();
        assert!(y.serves("ja") && !y.serves("en") && !y.serves("zh-Hans"));
        assert!(y.favours("ja") && !bing.favours("ja") && bing.serves("ko"));
    }

    #[test]
    fn validate_rejects_unsafe_or_broken_definitions() {
        let mut e = ddg();
        e.url = "http://insecure.example/?q={q}".into();
        assert!(e.validate().is_err());
        let mut e = ddg();
        e.container = "div[".into();
        assert!(e.validate().is_err());
        let mut e = ddg();
        e.id = "../x".into();
        assert!(e.validate().is_err());
    }
}
