//! 毎朝の点検と、AI による検索元の自動保守。
//!
//! 検索元が結果ページの作りを変えると、CSS セレクタが合わなくなり結果が0件になる。
//! 毎朝(と起動時に)全ての検索元で「見本の検索」を行い、読み取れなかった検索元があれば、
//! 最新のページの構造を aruaru-llm(無料の AI)に見せて、新しいセレクタを提案してもらう。
//!
//! **AI の提案は、そのまま使わない。** 次を全て満たしたときだけ取り込む:
//! 1. 変えてよいのは、セレクタ(container/title/link/snippet)と転送用パラメータ名(unwrap)だけ。
//!    URL・重みは変えない(AI に取得先を変えさせない)。
//! 2. 設定として正しい(`EngineDef::validate`)。
//! 3. 取得済みの実際のページで、3件以上の結果(http/https のリンクとタイトルつき)が読み取れる。
//!
//! **無効化は、AI の1回の回答だけでは行わない。** AI が「セレクタでは直せない」
//! (`{"give_up":true}`、CAPTCHA・JavaScript必須・利用拒否などセレクタの修正では
//! どうにもならないとき)と判断した回数を `EngineDef::give_up_streak` に積み、
//! `DISABLE_AFTER_GIVE_UPS`(既定3)回連続になった時点で初めて自動的に無効化する
//! (`selfcheck`)。直せた・正常に読み取れた時点で streak は 0 に戻るため、1回の
//! 一時的な失敗だけで止まることはない。無効化した経緯も `maintenance.log` に残る。
//!
//! 取り込んだ・断った・無効化した経緯は `maintenance.log`(1行1件の JSON)に残す。

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
    ("서울 맛집", "ko", "KR"),
    ("北京 温泉", "zh-CN", "CN"),
];

/// この検索元の点検に使う見本の検索。得意な言語が決まっている検索元(日本向け・韓国向け・中国向け)は、その言語で。
/// 全言語向けの検索元は、英語と日本語で。
pub fn canaries_for(def: &EngineDef) -> Vec<(&'static str, &'static str, &'static str)> {
    CANARIES
        .iter()
        .copied()
        .filter(|(_, hl, _)| {
            let code = crate::lang::resolve(hl, "").code;
            if def.langs.is_empty() {
                matches!(code, "en" | "ja")
            } else {
                def.favours(code)
            }
        })
        .collect()
}
const MIN_HITS: usize = 3;
/// この回数だけ連続で AI が「セレクタでは直せない」(give up)と判断したら、検索元を自動で無効化する。
pub const DISABLE_AFTER_GIVE_UPS: u32 = 3;

/// AI が「セレクタの修正では直せない」と回答したことを表す印(`anyhow::Error::downcast_ref` で見分ける)。
#[derive(Debug)]
pub struct GiveUp(pub String);
impl std::fmt::Display for GiveUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for GiveUp {}
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
         - link_attr: URL が href ではなく別の属性(例 data-href)にあるときはその名前、container 自身の属性にあるときは @名前(例 @mu)。通常は空文字
         - unwrap: link の href が転送URLのとき、本来のURLが入っているクエリ名(例 uddg)。不要なら空文字\n\
         - セレクタは標準的な CSS(:has や :not は使ってよい)。JavaScript が必要でページに結果が無いときは {{\"give_up\":true}} だけ返す\n\
         出力は JSON オブジェクト1つだけ(説明・コードブロック不要):\n\
         {{\"container\":\"...\",\"title\":\"...\",\"link\":\"...\",\"snippet\":\"...\",\"link_attr\":\"\",\"unwrap\":\"\"}}\n\n\
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
        return Err(GiveUp(
            "AI は読み取れないと判断しました(JavaScript が必要なページ、または利用を断られている可能性)"
                .to_string(),
        )
        .into());
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
    if let Some(a) = s("link_attr") {
        cand.link_attr = a;
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

/// 保守の経緯を、ファイルと(あれば)GitHub の保存先に残す。
pub async fn record(s: &Searcher, dir: &Path, engine: &str, outcome: &str, detail: &str) {
    log_line(dir, engine, outcome, detail);
    let store = s.store.read().ok().and_then(|g| g.clone());
    if let Some(st) = store {
        if let Err(e) = st.log(now_unix(), engine, outcome, detail).await {
            eprintln!("aruaru-search: {e:#}");
        }
    }
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

/// 保存済みの設定に、既定の設定を合わせる: 保存に無い検索元(新しく追加したもの)を足し、保存の版が既定より古いものは
/// 既定に置き換える(コードで直した設定を反映するため)。AI が直した設定は、版が同じなら残す。保存だけにある検索元も残す。
pub fn merge_defaults(saved: Vec<EngineDef>) -> Vec<EngineDef> {
    let mut out: Vec<EngineDef> = Vec::new();
    for d in engine::defaults() {
        match saved.iter().find(|s| s.id == d.id) {
            Some(s) if s.rev >= d.rev => out.push(s.clone()),
            _ => out.push(d),
        }
    }
    for s in saved {
        if !out.iter().any(|e| e.id == s.id) {
            out.push(s);
        }
    }
    out
}

pub fn load_engines(dir: &Path) -> Vec<EngineDef> {
    let saved: Option<Vec<EngineDef>> = std::fs::read(dir.join("engines.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    match saved {
        Some(v) if !v.is_empty() && v.iter().all(|e| e.validate().is_ok()) => merge_defaults(v),
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

/// `repair` の結果。`GiveUp` は「セレクタの修正ではどうにもならない」という AI の判断
/// (または AI に見せる中身すら無いこと)を表し、`selfcheck` が `give_up_streak` を積む材料にする。
pub enum RepairOutcome {
    Fixed,
    GiveUp,
    Failed,
}

/// 壊れた検索元を1つ直す。
pub async fn repair(
    s: &Searcher,
    http: &reqwest::Client,
    llm_base: &str,
    dir: &Path,
    def: &EngineDef,
    html: &str,
    query: &str,
) -> RepairOutcome {
    let structure = condense(html);
    if structure.len() < 200 {
        record(
            s,
            dir,
            &def.id,
            "give_up",
            "ページの内容が空に近く、AI に見せられません(CAPTCHA・利用拒否などの可能性)",
        )
        .await;
        return RepairOutcome::GiveUp;
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
            let snapshot: Vec<EngineDef> = match s.engines.write() {
                Ok(mut engines) => {
                    if let Some(slot) = engines.iter_mut().find(|e| e.id == def.id) {
                        *slot = cand.clone();
                    }
                    if let Err(e) = save_engines(dir, &engines) {
                        eprintln!("aruaru-search: 設定の保存に失敗: {e:#}");
                    }
                    engines.clone()
                }
                Err(_) => Vec::new(),
            };
            let store = s.store.read().ok().and_then(|g| g.clone());
            if let (Some(st), false) = (store, snapshot.is_empty()) {
                let msg = format!("aruaru-search repair {}", def.id);
                match st.save_engines(&snapshot, now_unix(), &msg).await {
                    Ok(commit) => {
                        eprintln!(
                            "aruaru-search: 設定を GitHub の保存先に保存しました(版 {commit})"
                        )
                    }
                    Err(e) => {
                        eprintln!("aruaru-search: GitHub の保存先への設定の保存に失敗: {e:#}")
                    }
                }
            }
            record(
                s,
                dir,
                &def.id,
                "applied",
                &format!(
                    "{n}件を読み取れる設定に更新: container={} title={} link={} snippet={} unwrap={}",
                    cand.container, cand.title, cand.link, cand.snippet, cand.unwrap
                ),
            )
            .await;
            RepairOutcome::Fixed
        }
        Err(e) => {
            if e.downcast_ref::<GiveUp>().is_some() {
                record(s, dir, &def.id, "give_up", &format!("{e:#}")).await;
                RepairOutcome::GiveUp
            } else {
                record(s, dir, &def.id, "rejected", &format!("{e:#}")).await;
                RepairOutcome::Failed
            }
        }
    }
}

/// 今の検索元の一覧を、ローカルと(あれば)GitHub の保存先の両方に保存する。
async fn persist_engines(s: &Searcher, dir: &Path, commit_msg: &str) {
    let snapshot: Vec<EngineDef> = match s.engines.read() {
        Ok(engines) => engines.clone(),
        Err(_) => return,
    };
    if let Err(e) = save_engines(dir, &snapshot) {
        eprintln!("aruaru-search: 設定の保存に失敗: {e:#}");
    }
    let store = s.store.read().ok().and_then(|g| g.clone());
    if let Some(st) = store {
        match st.save_engines(&snapshot, now_unix(), commit_msg).await {
            Ok(commit) => {
                eprintln!("aruaru-search: 設定を GitHub の保存先に保存しました(版 {commit})")
            }
            Err(e) => eprintln!("aruaru-search: GitHub の保存先への設定の保存に失敗: {e:#}"),
        }
    }
}

/// `give_up_streak` を 0 に戻す(直った・正常に戻ったとき)。変化が無ければ何もしない。
async fn reset_give_up_streak(s: &Searcher, dir: &Path, id: &str) {
    let changed = match s.engines.write() {
        Ok(mut engines) => match engines.iter_mut().find(|e| e.id == id) {
            Some(slot) if slot.give_up_streak != 0 => {
                slot.give_up_streak = 0;
                true
            }
            _ => false,
        },
        Err(_) => false,
    };
    if changed {
        persist_engines(s, dir, &format!("aruaru-search reset give_up_streak {id}")).await;
    }
}

/// `give_up_streak` を1つ積む。`DISABLE_AFTER_GIVE_UPS` に達したら、その場で無効化する。
/// 戻り値は (積んだ後の streak, 今回無効化したか)。
async fn bump_give_up_streak(s: &Searcher, dir: &Path, id: &str) -> (u32, bool) {
    let mut disabled_now = false;
    let streak = match s.engines.write() {
        Ok(mut engines) => match engines.iter_mut().find(|e| e.id == id) {
            Some(slot) => {
                slot.give_up_streak += 1;
                if slot.give_up_streak >= DISABLE_AFTER_GIVE_UPS && slot.enabled {
                    slot.enabled = false;
                    disabled_now = true;
                }
                slot.give_up_streak
            }
            None => 0,
        },
        Err(_) => 0,
    };
    persist_engines(s, dir, &format!("aruaru-search give_up {id} (streak {streak})")).await;
    if disabled_now {
        record(
            s,
            dir,
            id,
            "disabled",
            &format!(
                "AI が {DISABLE_AFTER_GIVE_UPS}回連続でセレクタでは直せないと判断したため、自動的に無効化しました\
                 (CAPTCHA・利用拒否などの可能性。有効化するには手動で `enabled` を true に戻してください)"
            ),
        )
        .await;
    }
    (streak, disabled_now)
}

/// 全ての検索元を点検し、読み取れないものは AI で直す。結果の要約(1行ずつ)を返す。
/// AI が直した設定は、新しく取得したページでもう一度確かめ、読み取れなければ**元の設定に戻す**
/// (提案が手元の1枚のページにだけ合っていて、実際には使えない場合に、動いている検索元を壊さないため)。
/// AI が「セレクタでは直せない」と `DISABLE_AFTER_GIVE_UPS` 回連続で判断した検索元は、自動で無効化する。
pub async fn selfcheck(
    s: &Arc<Searcher>,
    http: &reqwest::Client,
    llm_base: &str,
    dir: &Path,
) -> Vec<String> {
    let defs: Vec<EngineDef> = s.engines.read().map(|e| e.clone()).unwrap_or_default();
    let mut report = Vec::new();
    for def in defs.iter().filter(|d| d.enabled) {
        let canaries = canaries_for(def);
        if canaries.is_empty() {
            continue;
        }
        let mut broken: Option<(String, String)> = None;
        let mut ok_count = 0;
        for (q, hl, gl) in &canaries {
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
            None => {
                report.push(format!("{}: 正常({ok_count}/{})", def.id, canaries.len()));
                reset_give_up_streak(s, dir, &def.id).await;
            }
            Some((html, q)) if !html.is_empty() => {
                let outcome = repair(s, http, llm_base, dir, def, &html, &q).await;
                if matches!(outcome, RepairOutcome::GiveUp) {
                    let (streak, disabled_now) = bump_give_up_streak(s, dir, &def.id).await;
                    report.push(if disabled_now {
                        format!(
                            "{}: 読み取れず → AI がセレクタでは直せないと判断({streak}回連続) → 自動的に無効化",
                            def.id
                        )
                    } else {
                        format!(
                            "{}: 読み取れず → AI がセレクタでは直せないと判断(give_up_streak={streak}/{DISABLE_AFTER_GIVE_UPS})",
                            def.id
                        )
                    });
                    continue;
                }
                if !matches!(outcome, RepairOutcome::Fixed) {
                    report.push(format!(
                        "{}: 読み取れず → 修正できず(maintenance.log を確認)",
                        def.id
                    ));
                    continue;
                }
                // 直した設定を、新しく取得したページでもう一度確かめる
                let (cq, chl, cgl) = canaries[0];
                let confirmed = match s.engine(&def.id) {
                    Some(d2) => s
                        .query_engine(&d2, cq, chl, cgl)
                        .await
                        .is_ok_and(|h| h.len() >= MIN_HITS),
                    None => false,
                };
                if confirmed {
                    report.push(format!("{}: 読み取れず → AI が設定を修正(確認 OK)", def.id));
                    reset_give_up_streak(s, dir, &def.id).await;
                } else {
                    // 元に戻す
                    if let Ok(mut engines) = s.engines.write() {
                        if let Some(slot) = engines.iter_mut().find(|e| e.id == def.id) {
                            *slot = def.clone();
                        }
                        let _ = save_engines(dir, &engines);
                    }
                    record(
                        s,
                        dir,
                        &def.id,
                        "rolled_back",
                        "AI が直した設定は、新しく取得したページでは読み取れなかったため、元の設定に戻しました",
                    )
                    .await;
                    report.push(format!(
                        "{}: AI の修正は確認できず、元の設定に戻しました",
                        def.id
                    ));
                }
            }
            Some(_) => {
                record(
                    s,
                    dir,
                    &def.id,
                    "unreachable",
                    "ページを取得できませんでした(接続失敗・拒否)",
                )
                .await;
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
    fn saved_configs_are_merged_with_new_and_fixed_defaults() {
        let defaults = engine::defaults();
        // 保存が古い(新しい検索元が無い・yahoo の版が古い・bing は AI が直した)
        let mut saved: Vec<EngineDef> = defaults
            .iter()
            .filter(|d| d.id != "naver")
            .cloned()
            .collect();
        for s in &mut saved {
            if s.id == "yahoo-jp" {
                s.rev = 0;
                s.container = "div.old".into();
            }
            if s.id == "bing" {
                s.container = "li.repaired-by-ai".into();
            }
        }
        saved.push(EngineDef {
            id: "custom".into(),
            ..defaults[0].clone()
        });
        let m = merge_defaults(saved);
        let get = |id: &str| m.iter().find(|e| e.id == id).unwrap();
        assert!(m.iter().any(|e| e.id == "naver"), "新しい検索元は足される");
        assert_ne!(
            get("yahoo-jp").container,
            "div.old",
            "版が古い設定は、直した既定に置き換わる"
        );
        assert_eq!(
            get("bing").container,
            "li.repaired-by-ai",
            "AI が直した設定は残る"
        );
        assert!(
            m.iter().any(|e| e.id == "custom"),
            "保存だけにある検索元も残る"
        );
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

    #[tokio::test]
    async fn give_up_streak_disables_after_threshold_and_resets_on_recovery() {
        let dir = std::env::temp_dir().join(format!("aruaru-search-test-gu-{}", now_unix()));
        let s = Searcher::new(engine::defaults()).unwrap();
        let id = "bing";
        assert!(s.engine(id).unwrap().enabled, "前提: 既定で有効な検索元を使う");
        // DISABLE_AFTER_GIVE_UPS 回未満なら、まだ有効なまま
        for n in 1..DISABLE_AFTER_GIVE_UPS {
            let (streak, disabled_now) = bump_give_up_streak(&s, &dir, id).await;
            assert_eq!(streak, n);
            assert!(!disabled_now);
            assert!(s.engine(id).unwrap().enabled, "しきい値未満では無効化しない");
        }
        // ちょうどしきい値に達したら、無効化される
        let (streak, disabled_now) = bump_give_up_streak(&s, &dir, id).await;
        assert_eq!(streak, DISABLE_AFTER_GIVE_UPS);
        assert!(disabled_now);
        assert!(!s.engine(id).unwrap().enabled, "しきい値に達したら無効化する");
        // 保存先にも反映されている
        assert!(
            !load_engines(&dir)
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .enabled
        );
        // 正常に戻れば streak は 0 に戻る(有効/無効はここでは変えない、手動で戻す想定)
        reset_give_up_streak(&s, &dir, id).await;
        assert_eq!(s.engine(id).unwrap().give_up_streak, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn give_up_proposal_is_distinguishable_from_other_rejections() {
        let d = ddg();
        let err = apply_proposal(&d, &serde_json::json!({"give_up":true})).unwrap_err();
        assert!(
            err.downcast_ref::<GiveUp>().is_some(),
            "give_up の提案は GiveUp として見分けられる"
        );
        let err2 = apply_proposal(&d, &serde_json::json!({"container":"article["})).unwrap_err();
        assert!(
            err2.downcast_ref::<GiveUp>().is_none(),
            "他の失敗は GiveUp と見分けられない"
        );
    }
}
