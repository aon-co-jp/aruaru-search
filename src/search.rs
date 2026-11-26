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

const API_UA: &str = "aruaru-search/1.0 (+https://github.com/aon-co-jp/aruaru-search)";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const MIN_INTERVAL: Duration = Duration::from_millis(1200);
const CACHE_TTL: Duration = Duration::from_secs(3600);
/// 拒否された検索元を休ませる時間。拒否が続くたびに長くする(20分 → 1時間 → 3時間 → 12時間 → 24時間)。
/// 相手に負担をかけず、ブロックを長引かせないため。成功したら段階を最初に戻す。
const COOLDOWN_STEPS: [u64; 5] = [20 * 60, 60 * 60, 3 * 3600, 12 * 3600, 24 * 3600];
/// 429(混雑・回数制限)で休ませる時間(5分 → 15分 → 30分 → 1時間 → 3時間)。拒否より軽い
const RATE_STEPS: [u64; 5] = [5 * 60, 15 * 60, 30 * 60, 3600, 3 * 3600];
/// ネットワーク障害(接続失敗・タイムアウト・5xx)で休ませる時間。相手の拒否ではないので短い
const NET_STEPS: [u64; 5] = [30, 60, 120, 300, 600];
/// 探りの検索語。秘密も利用者の検索語も含まない、固定の無害な1語
pub const PROBE_QUERY: &str = "wikipedia";
const CACHE_MAX: usize = 500;

/// 休止の理由。理由によって休む長さと探りの間隔を変える。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// 拒否(403・202・503・確認画面・ソフトブロック): 長く休む
    Rejected,
    /// 429: 中くらい
    RateLimited,
    /// ネットワーク障害: 短く休む
    Network,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Rejected => "rejected",
            BlockKind::RateLimited => "rate_limited",
            BlockKind::Network => "network",
        }
    }
    fn steps(self) -> &'static [u64; 5] {
        match self {
            BlockKind::Rejected => &COOLDOWN_STEPS,
            BlockKind::RateLimited => &RATE_STEPS,
            BlockKind::Network => &NET_STEPS,
        }
    }
    /// 最初の探りまでの間隔(失敗するたびに2倍、上限まで)
    fn probe_base(self) -> u64 {
        match self {
            BlockKind::Rejected => 10 * 60,
            BlockKind::RateLimited => 5 * 60,
            BlockKind::Network => 20,
        }
    }
    fn probe_cap(self) -> u64 {
        match self {
            BlockKind::Rejected => 2 * 3600,
            BlockKind::RateLimited => 30 * 60,
            BlockKind::Network => 2 * 60,
        }
    }
}

/// 探りの間隔: 基本間隔 × 2^失敗回数(上限つき)。
pub fn probe_delay(kind: BlockKind, failures: u32) -> u64 {
    kind.probe_base()
        .saturating_mul(1u64 << failures.min(20))
        .min(kind.probe_cap())
}

/// 休止中の検索元の状態(時刻はすべて UNIX 秒。時計は差し替えられる)
#[derive(Clone, Debug)]
struct Cool {
    until: u64,
    level: u32,
    kind: BlockKind,
    probe_failures: u32,
    next_probe: u64,
}

/// 取得の失敗の種類
enum Fail {
    Net(String),
    Http(u16),
}
/// 取得するページの最大長(これを超える分は読まない)
const PAGE_MAX: usize = 3_000_000;

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
    /// 拒否され続けている段階(0=拒否されていない、最大4)
    pub blocked_level: u32,
    /// 休止が終わる時刻(UNIX 秒)。休止していなければ 0
    pub cooldown_until_unix: u64,
    /// 休止の理由(rejected / rate_limited / network)。休止していなければ None
    pub cooldown_reason: Option<String>,
    /// 次に探りの検索を投げる時刻(UNIX 秒)。休止していなければ 0
    pub next_probe_unix: u64,
    /// 直近の探りの時刻(0=まだ探っていない)と結果
    pub last_probe_unix: u64,
    pub last_probe_ok: Option<bool>,
    pub last_probe_error: Option<String>,
    pub probe_failures: u32,
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
    cooldown: Mutex<HashMap<String, Cool>>,
    /// 現在の UNIX 秒を返す時計(試験で差し替えられる)
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// 意味による並べ替えに使う aruaru-llm の URL(Connect の `Rerank`)。None なら使わない
    rerank_base: RwLock<Option<String>>,
    /// 保存先(GitHub の非公開リポジトリ。設定と保守履歴。任意)
    pub store: RwLock<Option<Arc<crate::store::Store>>>,
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
        Self::with_clock(engines, Arc::new(now_unix))
    }

    pub fn with_clock(
        engines: Vec<EngineDef>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Arc<Searcher>> {
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
            clock,
            rerank_base: RwLock::new(None),
            store: RwLock::new(None),
            cache: Mutex::new(HashMap::new()),
        }))
    }

    pub fn now(&self) -> u64 {
        (self.clock)()
    }

    /// 休止中か(休止が終わる時刻がまだ先)。
    pub fn is_cooling(&self, id: &str) -> bool {
        let now = self.now();
        self.cooldown
            .lock()
            .ok()
            .and_then(|c| c.get(id).map(|x| x.until > now))
            .unwrap_or(false)
    }

    /// 検索元を休ませる。拒否や制限が続くほど休む時間を長くする(段階は成功すると最初に戻る)。
    /// 理由(拒否・429・ネットワーク)で休む長さが違い、休止中は探りの検索で自動復帰を試みる。
    fn block(&self, def: &EngineDef, kind: BlockKind, reason: &str, max_level: u32) {
        let now = self.now();
        let mut level = 0;
        let mut secs = kind.steps()[0];
        let mut next_probe = now + probe_delay(kind, 0);
        if let Ok(mut c) = self.cooldown.lock() {
            level = c
                .get(&def.id)
                .filter(|x| x.kind == kind)
                .map_or(0, |x| (x.level + 1).min(max_level));
            secs = kind.steps()[level as usize];
            next_probe = (now + probe_delay(kind, 0)).min(now + secs);
            c.insert(
                def.id.clone(),
                Cool { until: now + secs, level, kind, probe_failures: 0, next_probe },
            );
        }
        if let Ok(mut st) = self.status.write() {
            let s = st.entry(def.id.clone()).or_default();
            s.blocked_level = level;
            s.cooldown_until_unix = now + secs;
            s.cooldown_reason = Some(kind.as_str().to_string());
            s.next_probe_unix = next_probe;
            s.probe_failures = 0;
        }
        eprintln!(
            "aruaru-search: {} を休ませます({reason}、{}、段階 {level})。{}秒間は使いません(探りは{}秒後)",
            def.id,
            kind.as_str(),
            secs,
            next_probe.saturating_sub(now)
        );
    }

    /// 休止の状態を解く(探りの成功)。段階を1つ下げて残す(すぐ再発したときに最初から長く休まないため)。
    fn recover(&self, id: &str) {
        let mut level = 0;
        if let Ok(mut c) = self.cooldown.lock() {
            if let Some(x) = c.get_mut(id) {
                x.until = 0;
                x.level = x.level.saturating_sub(1);
                x.probe_failures = 0;
                x.next_probe = 0;
                level = x.level;
            }
        }
        if let Ok(mut st) = self.status.write() {
            let s = st.entry(id.to_string()).or_default();
            s.blocked_level = level;
            s.cooldown_until_unix = 0;
            s.cooldown_reason = None;
            s.next_probe_unix = 0;
            s.probe_failures = 0;
        }
    }

    /// 探りの検索を1回投げる。成功なら即復帰(段階を1つ下げる)、失敗なら指数バックオフで次を決める。
    /// 休止の期限は延ばさない(探りが拒否を深めないように)。
    pub async fn probe_engine(&self, def: &EngineDef) -> bool {
        let res = self.fetch_raw(def, PROBE_QUERY, "en", "").await;
        let outcome: Result<(), String> = match res {
            Ok(html) => {
                if !def.is_api() && is_challenge_page(&html) {
                    Err("人かどうかの確認画面".into())
                } else if engine::parse(def, &html).is_empty() {
                    Err("結果を読み取れませんでした".into())
                } else {
                    Ok(())
                }
            }
            Err(Fail::Net(e)) => Err(format!("ネットワーク障害: {e}")),
            Err(Fail::Http(c)) => Err(format!("HTTP {c}")),
        };
        let now = self.now();
        let ok = outcome.is_ok();
        let mut next = None;
        if ok {
            self.recover(&def.id);
        } else if let Ok(mut c) = self.cooldown.lock() {
            if let Some(x) = c.get_mut(&def.id) {
                x.probe_failures += 1;
                // 次の探りは期限より後にしない(満了時に必ず状況を確かめる)
                x.next_probe =
                    (now + probe_delay(x.kind, x.probe_failures)).min(x.until.max(now + 1));
                next = Some((x.next_probe, x.probe_failures));
            }
        }
        if let Ok(mut st) = self.status.write() {
            let s = st.entry(def.id.clone()).or_default();
            s.last_probe_unix = now;
            s.last_probe_ok = Some(ok);
            s.last_probe_error = outcome.as_ref().err().cloned();
            if ok {
                s.last_ok = Some(true);
                s.last_error = None;
                s.consecutive_failures = 0;
            } else if let Some((np, pf)) = next {
                s.next_probe_unix = np;
                s.probe_failures = pf;
            }
        }
        eprintln!(
            "aruaru-search: {} の探り: {}",
            def.id,
            match outcome {
                Ok(()) => "成功。復帰します".to_string(),
                Err(e) => format!("失敗({e})。バックオフします"),
            }
        );
        ok
    }

    /// 探りの時刻が来た休止中の検索元を探る(`only` があればその ID だけ)。探った ID と結果を返す。
    pub async fn probe_due(&self, only: Option<&[String]>) -> Vec<(String, bool)> {
        let now = self.now();
        let due: Vec<String> = match self.cooldown.lock() {
            Ok(mut c) => c
                .iter_mut()
                .filter(|(id, x)| {
                    x.until > now && x.next_probe <= now && only.map_or(true, |o| o.contains(id))
                })
                .map(|(id, x)| {
                    // 二重に探らないよう、結果が出るまで次の時刻を先へ送っておく
                    x.next_probe = now + 60;
                    id.clone()
                })
                .collect(),
            Err(_) => return Vec::new(),
        };
        let defs: Vec<EngineDef> = due
            .iter()
            .filter_map(|id| self.engine(id))
            .filter(|d| d.enabled)
            .collect();
        futures::future::join_all(
            defs.iter()
                .map(|d| async move { (d.id.clone(), self.probe_engine(d).await) }),
        )
        .await
    }

    /// 全体の健康状態: 使える検索元の数と、休止中の検索元。
    pub fn health(&self) -> serde_json::Value {
        let now = self.now();
        let engines = self.engines.read().map(|e| e.clone()).unwrap_or_default();
        let status = self.status.read().map(|s| s.clone()).unwrap_or_default();
        let mut healthy = 0;
        let mut total = 0;
        let mut list = Vec::new();
        for e in engines.iter().filter(|e| e.enabled) {
            total += 1;
            let s = status.get(&e.id).cloned().unwrap_or_default();
            let cooling = s.cooldown_until_unix > now;
            let ok = s.last_ok != Some(false) && !cooling;
            if ok {
                healthy += 1;
            }
            list.push(serde_json::json!({
                "id": e.id, "ok": ok, "blocked_level": s.blocked_level,
                "cooldown_secs": s.cooldown_until_unix.saturating_sub(now),
                "last_error": s.last_error,
                "cooldown_reason": if cooling { s.cooldown_reason.clone() } else { None },
                "next_probe_unix": if cooling { s.next_probe_unix } else { 0 },
                "next_probe_secs": if cooling { s.next_probe_unix.saturating_sub(now) } else { 0 },
                "probe_failures": s.probe_failures,
                "last_probe": if s.last_probe_unix > 0 {
                    serde_json::json!({ "unix": s.last_probe_unix, "ok": s.last_probe_ok, "error": s.last_probe_error })
                } else { serde_json::Value::Null },
            }));
        }
        let state = if healthy == 0 {
            "down"
        } else if healthy < 2 {
            "degraded"
        } else {
            "ok"
        };
        serde_json::json!({ "status": state, "healthy": healthy, "total": total, "engines": list })
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
    async fn polite_wait(&self, id: &str, interval: Duration) {
        // 一定の間隔だと機械と見分けられやすいので、間隔にゆらぎ(0.8〜1.6倍)を付ける
        let interval = jitter(interval);
        let wait = {
            let mut m = self.last_call.lock().expect("lock");
            let now = Instant::now();
            let next = m.get(id).map_or(now, |t| (*t + interval).max(now));
            m.insert(id.to_string(), next);
            next.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// 休止を見ずに、1回だけ取得する(失敗の種類を返す。休止の記録はしない)。
    async fn fetch_raw(&self, def: &EngineDef, q: &str, hl: &str, gl: &str) -> Result<String, Fail> {
        let interval = if def.interval_ms > 0 {
            Duration::from_millis(def.interval_ms)
        } else {
            MIN_INTERVAL
        };
        self.polite_wait(&def.id, interval).await;
        let mut req = self
            .http
            .get(def.search_url(q, hl, gl))
            .header("Accept-Language", if hl.is_empty() { "en" } else { hl });
        req = if def.is_api() {
            // 公式 API は、連絡先の分かる名乗りを求められる(Wikimedia のポリシー等)ので、ブラウザを装わない
            req.header("User-Agent", API_UA).header("Accept", "application/json")
        } else {
            req.header("Accept", "text/html,application/xhtml+xml")
        };
        let resp = req.send().await.map_err(|e| Fail::Net(format!("{e}")))?;
        let status = resp.status();
        // 202 は検索元の「機械による利用の確認」(bot 対策)のページ。結果ではないので、ページの作りの変化とは区別する
        if !status.is_success() || status.as_u16() == 202 {
            return Err(Fail::Http(status.as_u16()));
        }
        let mut html = resp.text().await.map_err(|e| Fail::Net(format!("{e}")))?;
        // 極端に大きいページだけ切る(Yahoo! JAPAN などは結果の前に大きな CSS があり、数百 KB になる)
        if html.len() > PAGE_MAX {
            let mut cut = PAGE_MAX;
            while !html.is_char_boundary(cut) {
                cut -= 1;
            }
            html.truncate(cut);
        }
        Ok(html)
    }

    /// 1つの検索元から結果ページを取得する(解析はしない)。
    pub async fn fetch_page(&self, def: &EngineDef, q: &str, hl: &str, gl: &str) -> Result<String> {
        let now = self.now();
        if let Some(until) = self
            .cooldown
            .lock()
            .ok()
            .and_then(|c| c.get(&def.id).map(|x| x.until))
        {
            if until > now {
                bail!(
                    "休止中(直前に拒否されたため、{}分ほど間を空けます)",
                    (until - now) / 60 + 1
                );
            }
        }
        match self.fetch_raw(def, q, hl, gl).await {
            Ok(h) => Ok(h),
            Err(Fail::Net(e)) => {
                self.block(def, BlockKind::Network, "ネットワーク障害", 4);
                bail!("接続できませんでした({e})")
            }
            Err(Fail::Http(code)) => {
                // API は、無料枠の超過を 400(Stack Exchange)で返すことがある。続けて呼ばず、長めに休む
                if code == 429 {
                    self.block(def, BlockKind::RateLimited, "HTTP 429", 4);
                } else if matches!(code, 202 | 403 | 503) || (def.is_api() && code == 400) {
                    self.block(def, BlockKind::Rejected, &format!("HTTP {code}"), 4);
                } else if matches!(code, 500 | 502 | 504) {
                    self.block(def, BlockKind::Network, &format!("HTTP {code}"), 4);
                }
                bail!("拒否されました(HTTP {code}。混み合い・機械利用の制限の可能性)")
            }
        }
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
                let mut hits = engine::parse(def, &html);
                // 「人かどうかの確認」画面は、ページの作りの変化ではなく拒否。確認の突破は行わず、休ませる
                if !def.is_api() && hits.is_empty() && is_challenge_page(&html) {
                    self.block(def, BlockKind::Rejected, "人かどうかの確認画面", 4);
                    let e = "人かどうかの確認画面が返されました(機械利用を制限されています。確認の突破は行わず、休みます)".to_string();
                    if let Ok(mut st) = self.status.write() {
                        let s = st.entry(def.id.clone()).or_default();
                        s.last_checked_unix = self.now();
                        s.last_hits = 0;
                        s.last_ok = Some(false);
                        s.consecutive_failures += 1;
                        s.last_error = Some(e.clone());
                    }
                    bail!("{}: {e}", def.name);
                }
                let mut err = hits.is_empty().then(|| {
                    "結果を1件も読み取れませんでした(ページの作りが変わった可能性)".to_string()
                });
                // 検索語の一部(先頭だけ・末尾だけ)しか反映されない結果は、機械利用を疑われて機能を落とされた
                // 応答(いわゆる「ソフトブロック」)。誤った結果を返さず、その検索元を休ませる。
                if err.is_none() && !def.is_api() && !covers_query(q, &hits) {
                    // 拒否より軽い制限なので、休むのは最長3時間(段階2)まで
                    self.block(def, BlockKind::Rejected, "検索語の一部しか反映されない結果", 2);
                    hits.clear();
                    err = Some("検索語の一部しか反映されない結果でした(機械利用を疑われて制限されている可能性)".to_string());
                }
                (hits, err, Some(html))
            }
            Err(e) => (Vec::new(), Some(format!("{e:#}")), None),
        };
        if let Ok(mut st) = self.status.write() {
            let s = st.entry(def.id.clone()).or_default();
            s.last_checked_unix = self.now();
            s.last_hits = hits.len();
            s.last_ok = Some(err.is_none());
            if err.is_none() {
                s.consecutive_failures = 0;
                s.blocked_level = 0;
                s.cooldown_until_unix = 0;
                s.cooldown_reason = None;
                s.next_probe_unix = 0;
                s.probe_failures = 0;
                if let Ok(mut c) = self.cooldown.lock() {
                    c.remove(&def.id);
                }
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
        let mut defs = defs;
        // 言語向けの検索元がすべて休止中なら、復帰を待たず、探りの時刻が来たものを今すぐ探り、
        // それでもだめなら言語を限らない他の有効な検索元へ素早く切り替える
        if defs.iter().all(|d| self.is_cooling(&d.id)) {
            let ids: Vec<String> = defs.iter().map(|d| d.id.clone()).collect();
            let _ = tokio::time::timeout(Duration::from_secs(25), self.probe_due(Some(&ids))).await;
            if defs.iter().all(|d| self.is_cooling(&d.id)) {
                let others: Vec<EngineDef> = self
                    .engines
                    .read()
                    .map(|e| {
                        e.iter()
                            .filter(|d| d.enabled && !ids.contains(&d.id) && !self.is_cooling(&d.id))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                defs.extend(others);
            }
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
                .post(format!("{}/aruaru_llm.Llm/Rerank", base.trim_end_matches('/')))
                .header("x-tenant", "aruaru-search")
                .json(&serde_json::json!({ "query": q, "documents": docs }))
                .timeout(RERANK_TIMEOUT)
                .send()
                .await?;
            if !resp.status().is_success() {
                bail!("HTTP {}", resp.status().as_u16());
            }
            let body = resp.text().await?;
            let j: serde_json::Value = rust_json::secure::from_str_secure(&body, &rust_json::secure::Limits { max_bytes: 4 << 20, ..Default::default() })
                .map_err(|e| anyhow::anyhow!("応答を読めません: {e}"))?;
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

/// 間隔にゆらぎ(0.8〜1.6倍)を付ける。乱数のライブラリは使わず、時刻の細かい部分から作る。
pub fn jitter(d: Duration) -> Duration {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.subsec_nanos());
    let permille = 800 + u64::from(n % 800); // 800..1599
    Duration::from_millis(d.as_millis() as u64 * permille / 1000)
}

/// 検索元が返す「人かどうかの確認」画面(CAPTCHA・安全確認)か。短いページで、題名か本文にその語がある。
pub fn is_challenge_page(html: &str) -> bool {
    if html.len() > 20_000 {
        return false;
    }
    let l = html.to_lowercase();
    [
        "百度安全验证",
        "安全验证",
        "captcha",
        "unusual traffic",
        "are you a robot",
        "verify you are human",
        "アクセスが集中",
        "ロボットではありません",
    ]
    .iter()
    .any(|k| l.contains(&k.to_lowercase()))
}

/// 検索語の切り出し(空白区切り。1文字の語・記号だけの語は除く)
fn query_tokens(q: &str) -> Vec<String> {
    let mut v: Vec<String> = q
        .split_whitespace()
        .map(|s| {
            s.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|s| s.chars().count() >= 2)
        .collect();
    v.dedup();
    v
}

/// 結果が検索語全体を反映しているか。検索語が2語以上のとき、上位の結果のうち**2つ以上の語**を含むものが
/// 1件でもあればよい(全部の語を含むことまでは求めない)。1つも無ければ、片方の語だけで検索された疑い。
/// (2026-09-25 実測: 多数の検索のあと、Bing が「岩手県 渓流釣り」を「岩手県」だけで検索した結果を返した)
pub fn covers_query(q: &str, hits: &[Hit]) -> bool {
    let tokens = query_tokens(q);
    if tokens.len() < 2 || hits.len() < 3 {
        return true;
    }
    hits.iter().take(10).any(|h| {
        let text: String = format!("{}{}", h.title, h.snippet)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase();
        tokens.iter().filter(|t| text.contains(t.as_str())).count() >= 2
    })
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
    fn cooldown_steps_grow_and_stay_bounded() {
        assert!(
            COOLDOWN_STEPS.windows(2).all(|w| w[0] < w[1]),
            "拒否が続くほど長く休む"
        );
        assert_eq!(COOLDOWN_STEPS.len(), 5, "段階は 0〜4");
        assert_eq!(COOLDOWN_STEPS[0], 20 * 60);
        assert_eq!(COOLDOWN_STEPS[4], 24 * 3600);
    }

    #[test]
    fn results_covering_only_part_of_the_query_are_detected() {
        let h = |t: &str, s: &str| hit(t, "https://x.example/", s);
        // 「岩手県」だけを反映した結果(実測した劣化の例)
        let degraded = vec![
            h("岩手県ホームページ トップページ", ""),
            h("岩手県 - Wikipedia", "岩手県は東北地方の県"),
            h("【岩手県】観光スポットおすすめ23選", ""),
        ];
        assert!(!covers_query(
            "岩手県 渓流釣り 釣り場 遊漁券 体験",
            &degraded
        ));
        // 両方の語を含む結果が1件でもあれば正常
        let mut ok = degraded.clone();
        ok.push(h("岩手県の渓流釣り 遊漁券のご案内", ""));
        assert!(covers_query("岩手県 渓流釣り 釣り場 遊漁券 体験", &ok));
        // 1語の検索・結果が少ないときは判定しない
        assert!(covers_query("温泉", &degraded));
        assert!(covers_query("岩手県 渓流釣り", &degraded[..2]));
        // 語の切り出し
        assert_eq!(query_tokens("山梨県  温泉 a"), vec!["山梨県", "温泉"]);
    }

    #[test]
    fn jitter_stays_within_bounds() {
        for _ in 0..50 {
            let d = jitter(Duration::from_millis(1000));
            assert!((800..1600).contains(&(d.as_millis() as u64)), "{d:?}");
        }
    }

    #[test]
    fn challenge_pages_are_detected_but_real_results_are_not() {
        assert!(is_challenge_page(
            "<html><head><title>百度安全验证</title></head><body>请完成验证</body></html>"
        ));
        assert!(is_challenge_page("<title>Please verify</title> CAPTCHA"));
        let big = format!("<html>{}captcha</html>", "x".repeat(30_000));
        assert!(
            !is_challenge_page(&big),
            "大きな結果ページは確認画面ではない"
        );
        assert!(!is_challenge_page(
            "<html><title>北京 温泉_百度搜索</title></html>"
        ));
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

    // ---- 自動再開(探りの検索)の試験。時計と検索元(手元の模擬サーバー)を差し替える ----

    use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

    const OK_HTML: &str = r#"<html><body><div class="result results_links web-result"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa">Example A</a></h2><a class="result__snippet">About A</a></div></body></html>"#;

    /// 模擬の検索元。mode: 0=429, 1=403, 2=200(結果あり), 3=接続を切る(ネットワーク障害)
    async fn mock(mode: Arc<AtomicU8>) -> (u16, Arc<AtomicU64>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicU64::new(0));
        let h2 = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else { return };
                h2.fetch_add(1, Ordering::SeqCst);
                let m = mode.load(Ordering::SeqCst);
                tokio::spawn(async move {
                    if m == 3 {
                        return; // 何も返さず切る
                    }
                    let mut buf = [0u8; 4096];
                    let _ = c.read(&mut buf).await;
                    let (line, body) = match m {
                        0 => ("429 Too Many Requests", ""),
                        1 => ("403 Forbidden", ""),
                        _ => ("200 OK", OK_HTML),
                    };
                    let r = format!(
                        "HTTP/1.1 {line}\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = c.write_all(r.as_bytes()).await;
                });
            }
        });
        (port, hits)
    }

    fn mock_engine(port: u16) -> EngineDef {
        let mut d = engine::defaults()
            .into_iter()
            .find(|e| e.id == "duckduckgo")
            .unwrap();
        d.url = format!("http://127.0.0.1:{port}/?q={{q}}");
        d.interval_ms = 1;
        d.enabled = true;
        d
    }

    fn fake(t0: u64, engines: Vec<EngineDef>) -> (Arc<Searcher>, Arc<AtomicU64>) {
        let t = Arc::new(AtomicU64::new(t0));
        let t2 = t.clone();
        let s = Searcher::with_clock(engines, Arc::new(move || t2.load(Ordering::SeqCst))).unwrap();
        (s, t)
    }

    #[test]
    fn probe_delay_doubles_and_is_capped_per_reason() {
        assert_eq!(probe_delay(BlockKind::Rejected, 0), 600);
        assert_eq!(probe_delay(BlockKind::Rejected, 1), 1200);
        assert_eq!(probe_delay(BlockKind::Rejected, 2), 2400);
        assert_eq!(probe_delay(BlockKind::Rejected, 30), 7200, "上限あり・あふれない");
        assert_eq!(probe_delay(BlockKind::Network, 0), 20);
        assert_eq!(probe_delay(BlockKind::Network, 9), 120);
        // 理由ごとの休む長さ: ネットワーク障害 < 429 < 拒否
        for i in 0..5 {
            assert!(NET_STEPS[i] < RATE_STEPS[i] && RATE_STEPS[i] < COOLDOWN_STEPS[i]);
        }
    }

    #[tokio::test]
    async fn probe_success_recovers_immediately_before_cooldown_ends() {
        let mode = Arc::new(AtomicU8::new(1));
        let (port, _) = mock(mode.clone()).await;
        let def = mock_engine(port);
        let (s, t) = fake(1_000_000, vec![def.clone()]);
        assert!(s.fetch_page(&def, "x", "en", "").await.is_err());
        assert!(s.is_cooling("duckduckgo"));
        let h = s.health();
        assert_eq!(h["engines"][0]["cooldown_reason"], "rejected");
        assert_eq!(h["engines"][0]["cooldown_secs"], 1200);
        assert_eq!(h["engines"][0]["next_probe_secs"], 600);
        // 探りの時刻前は探らない
        t.store(1_000_000 + 599, Ordering::SeqCst);
        assert!(s.probe_due(None).await.is_empty());
        // 時刻が来て、相手が復帰していれば、休止の途中でも即復帰
        mode.store(2, Ordering::SeqCst);
        t.store(1_000_000 + 600, Ordering::SeqCst);
        assert_eq!(s.probe_due(None).await, vec![("duckduckgo".to_string(), true)]);
        assert!(!s.is_cooling("duckduckgo"));
        let h = s.health();
        assert_eq!(h["engines"][0]["ok"], true);
        assert_eq!(h["engines"][0]["last_probe"]["ok"], true);
        assert!(s.fetch_page(&def, "x", "en", "").await.is_ok(), "休止が解けて通常の検索に使える");
    }

    #[tokio::test]
    async fn probe_failure_backs_off_exponentially_without_extending_cooldown() {
        let mode = Arc::new(AtomicU8::new(1));
        let (port, _) = mock(mode.clone()).await;
        let def = mock_engine(port);
        let (s, t) = fake(5_000, vec![def.clone()]);
        // 拒否が続いて段階2(3時間)まで上がった状態を作る
        for _ in 0..3 {
            s.block(&def, BlockKind::Rejected, "試験", 4);
        }
        let until = 5_000 + COOLDOWN_STEPS[2];
        assert_eq!(s.status.read().unwrap()["duckduckgo"].cooldown_until_unix, until);
        let mut now = 5_000;
        for (fails, gap) in [(1u32, 1200u64), (2, 2400), (3, 4800), (4, 7200)] {
            now = s.status.read().unwrap()["duckduckgo"].next_probe_unix.max(now);
            t.store(now, Ordering::SeqCst);
            assert_eq!(s.probe_due(None).await, vec![("duckduckgo".to_string(), false)]);
            let st = s.status.read().unwrap()["duckduckgo"].clone();
            assert_eq!(st.probe_failures, fails);
            assert_eq!(st.next_probe_unix, (now + gap).min(until), "指数バックオフ(上限2時間、休止の満了時が上限)");
            assert_eq!(st.cooldown_until_unix, until, "探りの失敗で休止は延びない");
            assert_eq!(st.last_probe_ok, Some(false));
        }
    }

    #[tokio::test]
    async fn network_failure_is_short_and_classified_separately() {
        let mode = Arc::new(AtomicU8::new(3));
        let (port, _) = mock(mode.clone()).await;
        let def = mock_engine(port);
        let (s, t) = fake(10_000, vec![def.clone()]);
        assert!(s.fetch_page(&def, "x", "en", "").await.is_err());
        let st = s.status.read().unwrap()["duckduckgo"].clone();
        assert_eq!(st.cooldown_reason.as_deref(), Some("network"));
        assert_eq!(st.cooldown_until_unix, 10_000 + NET_STEPS[0]);
        assert_eq!(st.next_probe_unix, 10_000 + 20);
        // 429 は別の理由
        mode.store(0, Ordering::SeqCst);
        t.store(10_000 + 31, Ordering::SeqCst);
        assert!(s.fetch_page(&def, "x", "en", "").await.is_err());
        let st = s.status.read().unwrap()["duckduckgo"].clone();
        assert_eq!(st.cooldown_reason.as_deref(), Some("rate_limited"));
        assert_eq!(st.cooldown_until_unix, 10_031 + RATE_STEPS[0]);
    }

    #[tokio::test]
    async fn search_switches_to_other_engine_when_one_is_cooling() {
        let bad = Arc::new(AtomicU8::new(1));
        let good = Arc::new(AtomicU8::new(2));
        let (pb, _) = mock(bad).await;
        let (pg, hits_good) = mock(good).await;
        let mut a = mock_engine(pb);
        a.id = "a".into();
        let mut b = mock_engine(pg);
        b.id = "b".into();
        let (s, _t) = fake(20_000, vec![a, b]);
        let (r, _) = s.search("東京", "en", "US", 5).await.expect("b の結果が返る");
        assert!(!r.is_empty());
        assert!(s.is_cooling("a") && !s.is_cooling("b"));
        let before = hits_good.load(Ordering::SeqCst);
        let (r, _) = s.search("大阪", "en", "US", 5).await.expect("a が休止中でも結果が返る");
        assert!(!r.is_empty());
        assert!(hits_good.load(Ordering::SeqCst) > before);
        assert_eq!(PROBE_QUERY, "wikipedia");
    }
}
