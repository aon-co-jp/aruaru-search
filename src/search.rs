//! 複数の検索元へ同時に問い合わせ、結果を1つに統合する(APIキー不要)。
//!
//! 統合は順位の逆数を足し合わせる方式(RRF)。複数の検索元が上位に挙げたページほど上に来る。
//! 検索元ごとに最小の間隔を空け(相手に負担をかけない)、同じ検索は一定時間キャッシュする。
//! 失敗した検索元の状態は `Health` に記録し、毎朝の点検と AI による自動保守が使う。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::Serialize;

use crate::engine::{self, EngineDef, Hit};
use crate::lang::{self, Lang};

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const MIN_INTERVAL: Duration = Duration::from_millis(1200);
const CACHE_TTL: Duration = Duration::from_secs(3600);
/// 拒否された検索元を休ませる時間(相手に負担をかけず、ブロックを長引かせない)
const COOLDOWN: Duration = Duration::from_secs(20 * 60);
const CACHE_MAX: usize = 500;
/// 点検・保守で使う、最新の結果ページの見本(HTML)の最大長
pub const SAMPLE_MAX: usize = 300_000;

#[derive(Clone, Debug, Serialize)]
pub struct Merged {
    pub title: String,
    pub link: String,
    pub snippet: String,
    /// この結果を返した検索元
    pub engines: Vec<String>,
    pub score: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct EngineStatus {
    pub last_ok: Option<bool>,
    pub last_hits: usize,
    pub last_error: Option<String>,
    pub last_checked_unix: u64,
    pub consecutive_failures: u32,
    /// 直近に取得した結果ページ(保守用。外には出さない)
    #[serde(skip)]
    pub sample_html: String,
    /// 直近に使った検索語(見本の HTML を取ったときのもの)
    #[serde(skip)]
    pub sample_query: String,
}

pub struct Searcher {
    http: reqwest::Client,
    pub engines: RwLock<Vec<EngineDef>>,
    pub status: RwLock<HashMap<String, EngineStatus>>,
    last_call: Mutex<HashMap<String, Instant>>,
    /// 拒否(429・202・403 など)された検索元を、しばらく休ませる時刻
    cooldown: Mutex<HashMap<String, Instant>>,
    /// 意味による並べ替えに使う aruaru-llm の URL(`/v1/rerank`)。None なら使わない
    rerank_base: RwLock<Option<String>>,
    cache: Mutex<HashMap<String, (Instant, Vec<Merged>)>>,
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Searcher {
    pub fn new(engines: Vec<EngineDef>) -> Result<Arc<Searcher>> {
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .cookie_store(true)
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Arc::new(Searcher {
            http,
            engines: RwLock::new(engines),
            status: RwLock::new(HashMap::new()),
            last_call: Mutex::new(HashMap::new()),
            cooldown: Mutex::new(HashMap::new()),
            rerank_base: RwLock::new(None),
            cache: Mutex::new(HashMap::new()),
        }))
    }

    pub fn set_rerank(&self, base: Option<String>) {
        if let Ok(mut w) = self.rerank_base.write() {
            *w = base;
        }
    }

    pub fn engine(&self, id: &str) -> Option<EngineDef> {
        self.engines
            .read()
            .ok()?
            .iter()
            .find(|e| e.id == id)
            .cloned()
    }

    /// 相手に負担をかけないよう、同じ検索元への呼び出しの間隔を空ける。
    async fn polite_wait(&self, id: &str) {
        let wait = {
            let mut m = self.last_call.lock().expect("lock");
            let now = Instant::now();
            let next = m.get(id).map_or(now, |t| (*t + MIN_INTERVAL).max(now));
            m.insert(id.to_string(), next);
            next.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// 1つの検索元から結果ページを取得する(解析はしない)。
    pub async fn fetch_page(&self, def: &EngineDef, q: &str, hl: &str, gl: &str) -> Result<String> {
        if let Some(until) = self
            .cooldown
            .lock()
            .ok()
            .and_then(|c| c.get(&def.id).copied())
        {
            if until > Instant::now() {
                bail!(
                    "休止中(直前に拒否されたため、{}分ほど間を空けます)",
                    until.saturating_duration_since(Instant::now()).as_secs() / 60 + 1
                );
            }
        }
        self.polite_wait(&def.id).await;
        let resp = self
            .http
            .get(def.search_url(q, hl, gl))
            .header("Accept-Language", if hl.is_empty() { "en" } else { hl })
            .header("Accept", "text/html,application/xhtml+xml")
            .send()
            .await?;
        let status = resp.status();
        // 202 は検索元の「機械による利用の確認」(bot 対策)のページ。結果ではないので、ページの作りの変化とは区別する
        if !status.is_success() || status.as_u16() == 202 {
            if matches!(status.as_u16(), 202 | 403 | 429 | 503) {
                if let Ok(mut c) = self.cooldown.lock() {
                    c.insert(def.id.clone(), Instant::now() + COOLDOWN);
                }
            }
            bail!(
                "拒否されました(HTTP {}。混み合い・機械利用の制限の可能性)",
                status.as_u16()
            );
        }
        let mut html = resp.text().await?;
        if html.len() > SAMPLE_MAX {
            let mut cut = SAMPLE_MAX;
            while !html.is_char_boundary(cut) {
                cut -= 1;
            }
            html.truncate(cut);
        }
        Ok(html)
    }

    /// 1つの検索元を検索し、状態を記録する。
    pub async fn query_engine(
        &self,
        def: &EngineDef,
        q: &str,
        hl: &str,
        gl: &str,
    ) -> Result<Vec<Hit>> {
        let result = self.fetch_page(def, q, hl, gl).await;
        let (hits, err, sample) = match result {
            Ok(html) => {
                let hits = engine::parse(def, &html);
                let err = hits.is_empty().then(|| {
                    "結果を1件も読み取れませんでした(ページの作りが変わった可能性)".to_string()
                });
                (hits, err, Some(html))
            }
            Err(e) => (Vec::new(), Some(format!("{e:#}")), None),
        };
        if let Ok(mut st) = self.status.write() {
            let s = st.entry(def.id.clone()).or_default();
            s.last_checked_unix = now_unix();
            s.last_hits = hits.len();
            s.last_ok = Some(err.is_none());
            if err.is_none() {
                s.consecutive_failures = 0;
            } else {
                s.consecutive_failures += 1;
            }
            s.last_error = err.clone();
            if let Some(h) = sample {
                s.sample_html = h;
                s.sample_query = q.to_string();
            }
        }
        match err {
            None => Ok(hits),
            Some(e) => bail!("{}: {e}", def.name),
        }
    }

    /// 全ての有効な検索元で検索し、統合する。1つでも成功すれば結果を返す。
    pub async fn search(
        &self,
        q: &str,
        hl: &str,
        gl: &str,
        n: usize,
    ) -> Result<(Vec<Merged>, Vec<String>)> {
        let lang = lang::resolve(hl, gl);
        let (hl, gl) = (lang.hl.as_str(), lang.gl.as_str());
        let q = q.trim();
        if q.is_empty() || q.chars().count() > 300 {
            bail!("検索語は1〜300文字にしてください");
        }
        let n = n.clamp(1, 30);
        let key = format!("{q}\u{1}{hl}\u{1}{gl}\u{1}{n}");
        if let Ok(c) = self.cache.lock() {
            if let Some((t, v)) = c.get(&key) {
                if t.elapsed() < CACHE_TTL {
                    return Ok((v.clone(), Vec::new()));
                }
            }
        }
        let defs: Vec<EngineDef> = self
            .engines
            .read()
            .map(|e| {
                e.iter()
                    .filter(|d| d.enabled && d.serves(lang.code))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if defs.is_empty() {
            bail!("有効な検索元がありません");
        }
        let results = futures::future::join_all(
            defs.iter()
                .map(|d| async move { (d.clone(), self.query_engine(d, q, hl, gl).await) }),
        )
        .await;
        let mut warnings = Vec::new();
        let mut per_engine: Vec<(EngineDef, Vec<Hit>)> = Vec::new();
        for (d, r) in results {
            match r {
                Ok(h) => per_engine.push((d, h)),
                Err(e) => warnings.push(format!("{e:#}")),
            }
        }
        if per_engine.is_empty() {
            bail!(
                "どの検索元からも結果を得られませんでした: {}",
                warnings.join(" / ")
            );
        }
        let mut merged = merge(&per_engine, &lang, n.max(RERANK_TOP));
        if let Some(w) = self.semantic_rerank(q, &mut merged).await {
            warnings.push(w);
        }
        merged.truncate(n);
        if let Ok(mut c) = self.cache.lock() {
            if c.len() >= CACHE_MAX {
                c.clear();
            }
            c.insert(key, (Instant::now(), merged.clone()));
        }
        Ok((merged, warnings))
    }
}

/// 意味による並べ替えの対象にする上位の件数(aruaru-llm 側の上限に合わせる)
const RERANK_TOP: usize = 15;
/// 意味による並べ替えの待ち時間の上限。間に合わなければ、順位の統合だけの結果を返す
const RERANK_TIMEOUT: Duration = Duration::from_secs(12);

impl Searcher {
    /// aruaru-llm の多言語の埋め込み(open-cuda 上の multilingual-e5-small)で、検索語との
    /// 意味の近さを測り、順位に混ぜる。言語をまたいで比べられるので、英語圏に偏った結果や
    /// 検索語と関係の薄いページを下げられる。使えなくても検索は止めない(警告だけ返す)。
    async fn semantic_rerank(&self, q: &str, merged: &mut [Merged]) -> Option<String> {
        let base = self.rerank_base.read().ok()?.clone()?;
        if merged.len() < 3 {
            return None;
        }
        let top = merged.len().min(RERANK_TOP);
        let docs: Vec<String> = merged[..top]
            .iter()
            .map(|m| format!("{} {}", m.title, m.snippet))
            .collect();
        let call = async {
            let resp = self
                .http
                .post(format!("{}/v1/rerank", base.trim_end_matches('/')))
                .json(&serde_json::json!({ "query": q, "documents": docs }))
                .timeout(RERANK_TIMEOUT)
                .send()
                .await?;
            if !resp.status().is_success() {
                bail!("HTTP {}", resp.status().as_u16());
            }
            let j: serde_json::Value = resp.json().await?;
            let scores: Vec<f64> = j
                .get("scores")
                .and_then(|s| s.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                .unwrap_or_default();
            if scores.len() != top {
                bail!("応答の件数が合いません");
            }
            Ok(scores)
        };
        match call.await {
            Ok(scores) => {
                blend(&mut merged[..top], &scores);
                merged.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                None
            }
            Err(e) => Some(format!("意味による並べ替えは使えませんでした({e:#})")),
        }
    }
}

/// 順位の統合の点数(0.6)と、意味の近さ(0.4)を、それぞれ 0〜1 に正規化して混ぜる。
pub fn blend(items: &mut [Merged], sims: &[f64]) {
    let max_score = items.iter().map(|m| m.score).fold(f64::MIN, f64::max);
    let (lo, hi) = sims
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), &s| (l.min(s), h.max(s)));
    let span = (hi - lo).max(1e-9);
    for (m, &s) in items.iter_mut().zip(sims) {
        let rank_part = if max_score > 0.0 {
            m.score / max_score
        } else {
            0.0
        };
        m.score = 0.6 * rank_part + 0.4 * ((s - lo) / span);
    }
}

/// 検索結果の同一判定用に URL をそろえる(http/https・www・末尾の / ・#以降の違いを無視)
fn norm_url(u: &str) -> String {
    let u = u.to_ascii_lowercase();
    let u = u.split('#').next().unwrap_or(&u);
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let u = u.strip_prefix("www.").unwrap_or(u);
    u.trim_end_matches('/').to_string()
}

/// 順位の逆数の和(RRF、k=60)で統合する。
pub fn merge(per_engine: &[(EngineDef, Vec<Hit>)], lang: &Lang, n: usize) -> Vec<Merged> {
    let mut by_url: HashMap<String, Merged> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (def, hits) in per_engine {
        for (rank, h) in hits.iter().enumerate() {
            let key = norm_url(&h.link);
            let boost = if def.favours(lang.code) { 1.3 } else { 1.0 };
            let add = def.weight * boost / (60.0 + rank as f64 + 1.0);
            let e = by_url.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                Merged {
                    title: h.title.clone(),
                    link: h.link.clone(),
                    snippet: h.snippet.clone(),
                    engines: Vec::new(),
                    score: 0.0,
                }
            });
            e.score += add;
            if !e.engines.contains(&def.id) {
                e.engines.push(def.id.clone());
            }
            if e.snippet.chars().count() < h.snippet.chars().count() {
                e.snippet = h.snippet.clone();
            }
        }
    }
    let mut v: Vec<Merged> = order
        .into_iter()
        .filter_map(|k| by_url.remove(&k))
        .collect();
    // 探している言語の文字で書かれたページを上へ(英語・アメリカ中心の結果を避ける)
    for m in &mut v {
        m.score *= lang::rank_factor(lang.script, &m.title, &m.snippet);
    }
    v.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v.truncate(n);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: &str, w: f64) -> EngineDef {
        EngineDef {
            weight: w,
            ..engine::defaults()
                .into_iter()
                .next()
                .map(|mut d| {
                    d.id = id.into();
                    d
                })
                .unwrap()
        }
    }
    fn hit(t: &str, l: &str, s: &str) -> Hit {
        Hit {
            title: t.into(),
            link: l.into(),
            snippet: s.into(),
        }
    }

    #[test]
    fn pages_returned_by_several_engines_rank_first_and_urls_are_deduplicated() {
        let a = (
            def("a", 1.0),
            vec![
                hit("X", "https://x.example/", "s"),
                hit("Y", "https://y.example/1", "short"),
            ],
        );
        let b = (
            def("b", 1.0),
            vec![
                hit("Y2", "http://www.y.example/1#top", "a longer snippet"),
                hit("Z", "https://z.example/", ""),
            ],
        );
        let m = merge(&[a, b], &lang::resolve("en", ""), 10);
        assert_eq!(m.len(), 3);
        assert_eq!(
            m[0].link, "https://y.example/1",
            "2つの検索元が挙げたページが先頭"
        );
        assert_eq!(m[0].engines, vec!["a", "b"]);
        assert_eq!(m[0].snippet, "a longer snippet", "長いほうの要約を使う");
        assert_eq!(
            merge(
                &[(
                    def("a", 1.0),
                    vec![hit("X", "https://x/", ""), hit("Y", "https://y/", "")]
                )],
                &lang::resolve("en", ""),
                1
            )
            .len(),
            1
        );
    }

    #[test]
    fn japanese_queries_put_japanese_pages_above_english_ones() {
        let a = (
            def("a", 1.0),
            vec![
                hit(
                    "Hot springs in Yamanashi",
                    "https://en.example/",
                    "list of onsen",
                ),
                hit("山梨県の温泉一覧", "https://ja.example/", "日帰り温泉"),
            ],
        );
        let ja = merge(std::slice::from_ref(&a), &lang::resolve("ja", ""), 10);
        assert_eq!(
            ja[0].link, "https://ja.example/",
            "英語のページが1位でも、日本語の検索では日本語のページが先"
        );
        let en = merge(&[a], &lang::resolve("en", ""), 10);
        assert_eq!(
            en[0].link, "https://en.example/",
            "英語の検索では順位を変えない"
        );
    }

    #[test]
    fn blend_lets_meaning_lift_a_lower_ranked_but_relevant_page() {
        let mk = |t: &str, s: f64| Merged {
            title: t.into(),
            link: t.into(),
            snippet: String::new(),
            engines: vec![],
            score: s,
        };
        let mut v = vec![mk("off-topic", 0.033), mk("relevant", 0.030)];
        blend(&mut v, &[0.70, 0.90]);
        v.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        assert_eq!(v[0].title, "relevant");
        let mut same = vec![mk("a", 0.03), mk("b", 0.02)];
        blend(&mut same, &[0.8, 0.8]);
        assert!(
            same[0].score > same[1].score,
            "意味の近さが同じなら順位のまま"
        );
    }

    #[test]
    fn norm_url_ignores_scheme_www_slash_and_fragment() {
        assert_eq!(
            norm_url("HTTPS://www.Example.com/a/#x"),
            norm_url("http://example.com/a")
        );
        assert_ne!(
            norm_url("https://example.com/a"),
            norm_url("https://example.com/b")
        );
    }
}
