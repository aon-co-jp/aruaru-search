//! 毎朝の点検と、AI による検索元の自動保守。
//!
//! 検索元が結果ページの作りを変えると、CSS セレクタが合わなくなり結果が0件になる。
//! 毎朝(と起動時に)全ての検索元で「見本の検索」を行い、読み取れなかった検索元があれば、
//! 最新のページの構造を aruaru-llm(無料の AI)に見せて、新しいセレクタを提案してもらう。
//!
//! **AI の提案は、そのまま使わない。** 次を全て満たしたときだけ取り込む:
//! 1. 変えてよいのは、セレクタ(container/title/link/snippet)と転送用パラメータ名(unwrap)だけ。
//!    URL・重み・有効/無効は変えない(AI に取得先を変えさせない)。
//! 2. 設定として正しい(`EngineDef::validate`)。
//! 3. 取得済みの実際のページで、3件以上の結果(http/https のリンクとタイトルつき)が読み取れる。
//!
//! 取り込んだ・断った経緯は `maintenance.log`(1行1件の JSON)に残す。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use scraper::{ElementRef, Html, Node};
use serde_json::Value as Json;

use crate::engine::{self, EngineDef};
use crate::search::{now_unix, Searcher};

/// 見本の検索語(言語ごと)
pub const CANARIES: &[(&str, &str, &str)] = &[
    ("open source software", "en", "US"),
    ("山梨県 温泉", "ja", "JP"),
];
const MIN_HITS: usize = 3;
const HTML_BUDGET: usize = 14_000;

/// AI に見せるため、ページの構造を小さくまとめる(script/style/svg を除き、class・id・href だけ残す)。
pub fn condense(html: &str) -> String {
    fn walk(e: ElementRef<'_>, out: &mut String, depth: usize) {
        if out.len() > HTML_BUDGET * 4 || depth > 40 {
            return;
        }
        let name = e.value().name();
        if matches!(
            name,
            "script"
                | "style"
                | "svg"
                | "noscript"
                | "head"
                | "link"
                | "meta"
                | "path"
                | "img"
                | "iframe"
        ) {
            return;
        }
        out.push('<');
        out.push_str(name);
        for (k, v) in e.value().attrs() {
            if matches!(k, "class" | "id" | "href" | "data-type") {
                let v: String = v.chars().take(if k == "href" { 90 } else { 80 }).collect();
                out.push_str(&format!(" {k}=\"{v}\""));
            }
        }
        out.push('>');
        for c in e.children() {
            match c.value() {
                Node::Text(t) => {
                    let s: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !s.is_empty() {
                        out.extend(s.chars().take(80));
                    }
                }
                Node::Element(_) => {
                    if let Some(ce) = ElementRef::wrap(c) {
                        walk(ce, out, depth + 1);
                    }
                }
                _ => {}
            }
        }
        out.push_str("</");
        out.push_str(name);
        out.push('>');
    }
    let doc = Html::parse_document(html);
    let mut out = String::new();
    if let Ok(sel) = scraper::Selector::parse("body") {
        if let Some(b) = doc.select(&sel).next() {
            walk(b, &mut out, 0);
        }
    }
    // 結果が並ぶ中ほどを優先して、長すぎれば先頭側を落とす
    if out.chars().count() > HTML_BUDGET {
        let skip = (out.chars().count() - HTML_BUDGET) / 3;
        out = out.chars().skip(skip).take(HTML_BUDGET).collect();
    }
    out
}

pub fn prompt(def: &EngineDef, query: &str, structure: &str) -> String {
    format!(
        "あなたは検索エンジンの結果ページ(HTML)を読み取る CSS セレクタを直す保守担当です。\n\
         検索元「{name}」の結果ページで、今のセレクタでは結果を1件も読み取れなくなりました。\n\
         検索語は「{query}」です。次のページ構造(script/style を除き簡略化)を見て、新しいセレクタを返してください。\n\n\
         今の設定: container={container} / title={title} / link={link} / snippet={snippet} / unwrap={unwrap}\n\n\
         条件:\n\
         - container: 検索結果1件ぶんを囲む要素(広告・関連検索・「もっと見る」は含めない)\n\
         - title / link: container の中のタイトルとリンク(link は href を持つ a 要素)\n\
         - snippet: container の中の要約(無ければ空文字)\n\
         - unwrap: link の href が転送URLのとき、本来のURLが入っているクエリ名(例 uddg)。不要なら空文字\n\
         - セレクタは標準的な CSS(:has や :not は使ってよい)。JavaScript が必要でページに結果が無いときは {{\"give_up\":true}} だけ返す\n\
         出力は JSON オブジェクト1つだけ(説明・コードブロック不要):\n\
         {{\"container\":\"...\",\"title\":\"...\",\"link\":\"...\",\"snippet\":\"...\",\"unwrap\":\"\"}}\n\n\
         ページ構造:\n{structure}",
        name = def.name,
        container = def.container,
        title = def.title,
        link = def.link,
        snippet = def.snippet,
        unwrap = def.unwrap,
    )
}

/// AI の回答から最初の JSON オブジェクトを取り出す。
fn extract_json(text: &str) -> Option<Json> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// AI の提案を、今の設定に重ねた候補にする(変えてよい項目だけ)。
pub fn apply_proposal(current: &EngineDef, proposal: &Json) -> Result<EngineDef> {
    if proposal.get("give_up").and_then(Json::as_bool) == Some(true) {
        bail!("AI は読み取れないと判断しました(JavaScript が必要なページ、または利用を断られている可能性)");
    }
    let s = |k: &str| {
        proposal
            .get(k)
            .and_then(Json::as_str)
            .map(|s| s.trim().to_string())
    };
    let mut cand = current.clone();
    for (field, slot) in [
        ("container", &mut cand.container),
        ("title", &mut cand.title),
        ("link", &mut cand.link),
        ("snippet", &mut cand.snippet),
    ] {
        match s(field) {
            Some(v) if !v.is_empty() || field == "snippet" => *slot = v,
            _ => bail!("提案に {field} がありません"),
        }
    }
    if let Some(u) = s("unwrap") {
        if !u
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            bail!("unwrap の指定が正しくありません");
        }
        cand.unwrap = u;
    }
    cand.validate()?;
    Ok(cand)
}

/// 候補が、実際に取得したページで使えるか確かめる。
pub fn verify(cand: &EngineDef, html: &str) -> Result<usize> {
    let hits = engine::parse(cand, html);
    if hits.len() < MIN_HITS {
        bail!(
            "実際のページで {} 件しか読み取れませんでした(必要 {MIN_HITS} 件)",
            hits.len()
        );
    }
    let with_snip = hits.iter().filter(|h| !h.snippet.is_empty()).count();
    if !cand.snippet.is_empty() && with_snip == 0 {
        bail!("要約が1件も読み取れません");
    }
    if hits.iter().any(|h| h.title.chars().count() > 300) {
        bail!("タイトルが長すぎます(かたまりの指定が広すぎる可能性)");
    }
    Ok(hits.len())
}

pub fn log_line(dir: &Path, engine: &str, outcome: &str, detail: &str) {
    let line = serde_json::json!({ "unix": now_unix(), "engine": engine, "outcome": outcome, "detail": detail });
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("maintenance.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
}

pub fn save_engines(dir: &Path, engines: &[EngineDef]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("engines.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(engines)?)?;
    std::fs::rename(&tmp, dir.join("engines.json"))?;
    Ok(())
}

pub fn load_engines(dir: &Path) -> Vec<EngineDef> {
    let saved: Option<Vec<EngineDef>> = std::fs::read(dir.join("engines.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    match saved {
        Some(v) if !v.is_empty() && v.iter().all(|e| e.validate().is_ok()) => v,
        _ => engine::defaults(),
    }
}

/// aruaru-llm(無料 AI)へ1回問い合わせる。
async fn ask_ai(http: &reqwest::Client, llm_base: &str, prompt: &str) -> Result<String> {
    let resp = http
        .post(format!(
            "{}/v1/chat-providers/complete-priority",
            llm_base.trim_end_matches('/')
        ))
        .json(&serde_json::json!({ "prompt": prompt }))
        .timeout(Duration::from_secs(150))
        .send()
        .await
        .with_context(|| format!("aruaru-llm({llm_base})に接続できません"))?;
    let j: Json = resp.json().await.context("aruaru-llm の応答を読めません")?;
    j.get("reply")
        .and_then(|r| r.get("text"))
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("AI から回答を得られませんでした"))
}

/// 壊れた検索元を1つ直す。直せたら true。
pub async fn repair(
    s: &Searcher,
    http: &reqwest::Client,
    llm_base: &str,
    dir: &Path,
    def: &EngineDef,
    html: &str,
    query: &str,
) -> bool {
    let structure = condense(html);
    if structure.len() < 200 {
        log_line(
            dir,
            &def.id,
            "skipped",
            "ページの内容が空に近く、AI に見せられません(利用を断られている可能性)",
        );
        return false;
    }
    let outcome: Result<(EngineDef, usize)> = async {
        let reply = ask_ai(http, llm_base, &prompt(def, query, &structure)).await?;
        let proposal =
            extract_json(&reply).ok_or_else(|| anyhow!("AI の回答から設定を読み取れません"))?;
        let cand = apply_proposal(def, &proposal)?;
        let n = verify(&cand, html)?;
        Ok((cand, n))
    }
    .await;
    match outcome {
        Ok((cand, n)) => {
            if let Ok(mut engines) = s.engines.write() {
                if let Some(slot) = engines.iter_mut().find(|e| e.id == def.id) {
                    *slot = cand.clone();
                }
                if let Err(e) = save_engines(dir, &engines) {
                    eprintln!("aruaru-search: 設定の保存に失敗: {e:#}");
                }
            }
            log_line(
                dir,
                &def.id,
                "applied",
                &format!("{n}件を読み取れる設定に更新: container={} title={} link={} snippet={} unwrap={}", cand.container, cand.title, cand.link, cand.snippet, cand.unwrap),
            );
            true
        }
        Err(e) => {
            log_line(dir, &def.id, "rejected", &format!("{e:#}"));
            false
        }
    }
}

/// 全ての検索元を点検し、読み取れないものは AI で直す。結果の要約(1行ずつ)を返す。
pub async fn selfcheck(
    s: &Arc<Searcher>,
    http: &reqwest::Client,
    llm_base: &str,
    dir: &Path,
) -> Vec<String> {
    let defs: Vec<EngineDef> = s.engines.read().map(|e| e.clone()).unwrap_or_default();
    let mut report = Vec::new();
    for def in defs.iter().filter(|d| d.enabled) {
        let mut broken: Option<(String, String)> = None;
        let mut ok_count = 0;
        for (q, hl, gl) in CANARIES {
            match s.query_engine(def, q, hl, gl).await {
                Ok(h) if h.len() >= MIN_HITS => ok_count += 1,
                Ok(_) | Err(_) => {
                    let html = s
                        .status
                        .read()
                        .ok()
                        .and_then(|st| st.get(&def.id).map(|x| x.sample_html.clone()))
                        .unwrap_or_default();
                    broken.get_or_insert((html, q.to_string()));
                }
            }
        }
        match broken {
            None => report.push(format!("{}: 正常({ok_count}/{})", def.id, CANARIES.len())),
            Some((html, q)) if !html.is_empty() => {
                let fixed = repair(s, http, llm_base, dir, def, &html, &q).await;
                report.push(format!(
                    "{}: 読み取れず → {}",
                    def.id,
                    if fixed {
                        "AI が設定を修正"
                    } else {
                        "修正できず(maintenance.log を確認)"
                    }
                ));
                if fixed {
                    // 直した設定で、もう一度確かめる
                    if let Some(d2) = s.engine(&def.id) {
                        let again = s
                            .query_engine(&d2, CANARIES[0].0, CANARIES[0].1, CANARIES[0].2)
                            .await;
                        report.push(format!(
                            "{}: 修正後の確認 {}",
                            def.id,
                            if again.is_ok() { "OK" } else { "NG" }
                        ));
                    }
                }
            }
            Some(_) => {
                log_line(
                    dir,
                    &def.id,
                    "unreachable",
                    "ページを取得できませんでした(接続失敗・拒否)",
                );
                report.push(format!("{}: 取得できず(接続失敗または拒否)", def.id));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ddg() -> EngineDef {
        engine::defaults()
            .into_iter()
            .find(|e| e.id == "duckduckgo")
            .unwrap()
    }

    const CHANGED: &str = r#"<html><head><script>var x=1</script></head><body>
      <main>
      <article class="r-item"><h3><a class="r-link" href="https://a.example/1">Alpha</a></h3><p class="r-desc">about alpha</p></article>
      <article class="r-item"><h3><a class="r-link" href="https://b.example/2">Beta</a></h3><p class="r-desc">about beta</p></article>
      <article class="r-item"><h3><a class="r-link" href="https://c.example/3">Gamma</a></h3><p class="r-desc">about gamma</p></article>
      </main></body></html>"#;

    #[test]
    fn current_selectors_fail_on_a_redesigned_page_and_a_valid_proposal_fixes_it() {
        assert!(engine::parse(&ddg(), CHANGED).is_empty());
        let p: Json = serde_json::json!({"container":"article.r-item","title":"h3 a","link":"h3 a","snippet":"p.r-desc","unwrap":""});
        let cand = apply_proposal(&ddg(), &p).unwrap();
        assert_eq!(verify(&cand, CHANGED).unwrap(), 3);
        assert_eq!(cand.url, ddg().url, "取得先の URL は AI に変えさせない");
        assert_eq!(cand.weight, ddg().weight);
    }

    #[test]
    fn bad_proposals_are_rejected() {
        let d = ddg();
        // 読み取れない設定
        let p = serde_json::json!({"container":"div.nothing","title":"a","link":"a","snippet":"","unwrap":""});
        assert!(verify(&apply_proposal(&d, &p).unwrap(), CHANGED).is_err());
        // セレクタが不正
        let p = serde_json::json!({"container":"article[","title":"a","link":"a","snippet":"","unwrap":""});
        assert!(apply_proposal(&d, &p).is_err());
        // 必須項目が無い
        assert!(apply_proposal(&d, &serde_json::json!({"container":"article"})).is_err());
        // 諦める
        assert!(apply_proposal(&d, &serde_json::json!({"give_up":true})).is_err());
        // URL を書き換えようとしても無視される(項目として読まない)
        let p = serde_json::json!({"container":"article.r-item","title":"h3 a","link":"h3 a","snippet":"p","url":"https://evil.example/?q={q}"});
        assert_eq!(apply_proposal(&d, &p).unwrap().url, d.url);
        // unwrap に記号
        let p = serde_json::json!({"container":"article","title":"a","link":"a","snippet":"","unwrap":"x&y"});
        assert!(apply_proposal(&d, &p).is_err());
    }

    #[test]
    fn condense_drops_scripts_and_keeps_structure() {
        let c = condense(CHANGED);
        assert!(
            c.contains("article class=\"r-item\"") && c.contains("Alpha") && !c.contains("var x")
        );
    }

    #[test]
    fn extract_json_handles_fenced_answers() {
        let j = extract_json("説明です\n```json\n{\"container\":\"a\"}\n```").unwrap();
        assert_eq!(j["container"], "a");
        assert!(extract_json("none").is_none());
    }

    #[test]
    fn engines_round_trip_and_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("aruaru-search-test-{}", now_unix()));
        assert_eq!(load_engines(&dir), engine::defaults());
        let mut v = engine::defaults();
        v[0].container = "div.changed".into();
        save_engines(&dir, &v).unwrap();
        assert_eq!(load_engines(&dir)[0].container, "div.changed");
        std::fs::write(dir.join("engines.json"), b"not json").unwrap();
        assert_eq!(
            load_engines(&dir),
            engine::defaults(),
            "壊れた設定は既定へ戻す"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
