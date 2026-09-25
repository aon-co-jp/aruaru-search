//! aruaru-search: Rust 製の自前メタ検索。APIキー不要・VPS で完全無料。
//!
//! RPoem(`open-runo-poem-compat`)の上で JSON の検索 API を提供する。
//! - `GET /v1/search?q=...&hl=ja&gl=JP&n=10`(`POST /v1/search` に JSON も可)
//! - `GET /v1/engines`(検索元の設定と、最近の状態)
//! - `POST /admin/selfcheck`(`x-admin-token` が必要。環境変数 `ARUARU_SEARCH_ADMIN_TOKEN` を設定したときだけ有効)
//! - `GET /healthz`
//!
//! 毎朝7時(日本時間)と起動時に、全ての検索元を点検し、読み取れなくなったものは AI(aruaru-llm)で自動保守する。

mod archive;
mod engine;
mod lang;
mod languages;
mod limit;
mod maintain;
mod search;
mod store;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use http_body_util::{BodyExt, Limited};
use open_runo_poem_compat::hyper_compat::{html_response, json_response};
use open_runo_poem_compat::{
    get, handler_fn, post, Request, Response, Route, Server, StatusCode, TcpListener,
};
use serde_json::json;

use search::Searcher;

const INDEX_HTML: &str = include_str!("../web_index.html");

struct Ctx {
    searcher: Arc<Searcher>,
    http: reqwest::Client,
    llm_base: String,
    dir: PathBuf,
    admin_token: Option<String>,
    limiter: limit::Limiter,
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| pct_decode(v))
    })
}

fn bad(status: StatusCode, msg: &str) -> Response {
    json_response(status, &json!({ "error": msg }))
}

/// プロキシが付けるヘッダから、利用者の IP を取り出す(無ければ内部の利用)。
fn client_of(req: &Request) -> Option<String> {
    let h = |k: &str| req.headers().get(k).and_then(|v| v.to_str().ok());
    limit::client_key(h("x-forwarded-for"), h("x-real-ip"))
}

async fn do_search(
    ctx: &Ctx,
    client: Option<String>,
    q: &str,
    hl: &str,
    gl: &str,
    n: usize,
) -> Response {
    if let Err(wait) = ctx.limiter.check(client.as_deref(), search::now_unix()) {
        return json_response(
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "error": format!("利用回数の上限に達しました。{wait}秒後にもう一度お試しください"), "retry_after": wait }),
        );
    }
    let ok_tag = |s: &str, max: usize| {
        s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if !ok_tag(hl, 12) || !ok_tag(gl, 6) {
        return bad(StatusCode::BAD_REQUEST, "hl / gl が正しくありません");
    }
    match ctx.searcher.search(q, hl, gl, n).await {
        Ok((results, warnings)) => json_response(
            StatusCode::OK,
            &json!({ "results": results, "warnings": warnings }),
        ),
        Err(e) => bad(StatusCode::BAD_GATEWAY, &format!("{e:#}")),
    }
}

fn app(ctx: Arc<Ctx>) -> Route {
    let c1 = ctx.clone();
    let c2 = ctx.clone();
    let c3 = ctx.clone();
    let c4 = ctx.clone();
    let c5 = ctx.clone();
    Route::new()
        .at(
            "/",
            get(handler_fn(|_r, _p| async {
                html_response(StatusCode::OK, INDEX_HTML)
            })),
        )
        .at(
            "/healthz",
            get(handler_fn(|_r, _p| async {
                json_response(StatusCode::OK, &json!({ "ok": true }))
            })),
        )
        .at(
            "/v1/search",
            get(handler_fn(move |req: Request, _p| {
                let ctx = c1.clone();
                async move {
                    let client = client_of(&req);
                    let qs = req.uri().query().unwrap_or("").to_string();
                    let q = query_param(&qs, "q").unwrap_or_default();
                    let hl = query_param(&qs, "hl").unwrap_or_default();
                    let gl = query_param(&qs, "gl").unwrap_or_default();
                    let n = query_param(&qs, "n")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(10);
                    do_search(&ctx, client, &q, &hl, &gl, n).await
                }
            }))
            .post(handler_fn(move |req: Request, _p| {
                let ctx = c2.clone();
                async move {
                    let client = client_of(&req);
                    let Ok(body) = Limited::new(req.into_body(), 64 << 10).collect().await else {
                        return bad(StatusCode::PAYLOAD_TOO_LARGE, "リクエストが大きすぎます");
                    };
                    let Ok(j) = serde_json::from_slice::<serde_json::Value>(&body.to_bytes())
                    else {
                        return bad(StatusCode::BAD_REQUEST, "JSON が正しくありません");
                    };
                    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let n = j
                        .get("max_results")
                        .or_else(|| j.get("n"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(10) as usize;
                    // aruaru-llm の /v1/search/raw と同じ項目名(query / hl / gl / max_results)も受け付ける
                    let q = if s("q").is_empty() {
                        s("query")
                    } else {
                        s("q")
                    };
                    do_search(&ctx, client, &q, &s("hl"), &s("gl"), n).await
                }
            })),
        )
        .at(
            "/v1/health",
            get(handler_fn(move |_r, _p| {
                let ctx = c5.clone();
                async move { json_response(StatusCode::OK, &ctx.searcher.health()) }
            })),
        )
        .at(
            "/v1/engines",
            get(handler_fn(move |_r, _p| {
                let ctx = c3.clone();
                async move {
                    let engines = ctx
                        .searcher
                        .engines
                        .read()
                        .map(|e| e.clone())
                        .unwrap_or_default();
                    let status = ctx
                        .searcher
                        .status
                        .read()
                        .map(|s| s.clone())
                        .unwrap_or_default();
                    json_response(
                        StatusCode::OK,
                        &json!({ "engines": engines, "status": status }),
                    )
                }
            })),
        )
        .at(
            "/admin/selfcheck",
            post(handler_fn(move |req: Request, _p| {
                let ctx = c4.clone();
                async move {
                    let given = req
                        .headers()
                        .get("x-admin-token")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    match &ctx.admin_token {
                        Some(t) if !t.is_empty() && t == given => {}
                        _ => return bad(StatusCode::FORBIDDEN, "管理トークンが必要です"),
                    }
                    let report =
                        maintain::selfcheck(&ctx.searcher, &ctx.http, &ctx.llm_base, &ctx.dir)
                            .await;
                    json_response(StatusCode::OK, &json!({ "report": report }))
                }
            })),
        )
        .with_compression()
}

/// 日本時間の (年月日の通算日, 時)
fn jst_day_hour(unix: u64) -> (u64, u64) {
    let t = unix + 9 * 3600;
    (t / 86_400, (t % 86_400) / 3600)
}

async fn schedule(ctx: Arc<Ctx>) {
    // 起動して少し待ってから最初の点検(起動直後の混雑を避ける)
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
    let mut last_day = 0;
    loop {
        let (day, hour) = jst_day_hour(search::now_unix());
        if day != last_day && (hour >= 7 || last_day == 0) {
            last_day = day;
            let report =
                maintain::selfcheck(&ctx.searcher, &ctx.http, &ctx.llm_base, &ctx.dir).await;
            for line in report {
                println!("aruaru-search: 点検 {line}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind: SocketAddr = std::env::var("ARUARU_SEARCH_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4610".into())
        .parse()
        .expect("ARUARU_SEARCH_BIND は ホスト:ポート 形式で指定してください");
    let dir: PathBuf = std::env::var("ARUARU_SEARCH_DATA_DIR")
        .unwrap_or_else(|_| "data".into())
        .into();
    let mut engines = maintain::load_engines(&dir);
    // 保存先(GitHub の非公開リポジトリ、任意): 保存済みの設定があり、ファイルの設定が無ければそれを使う(VPS の作り直しでも設定を戻せる)
    let mut store = None;
    if let Some(repo) = archive::repo_from_env() {
        match store::Store::connect(&repo).await {
            Ok(s) => {
                match s.load_engines().await {
                    Ok(saved) if !saved.is_empty() && !dir.join("engines.json").exists() => {
                        println!(
                            "aruaru-search: GitHub の保存先から検索元の設定 {} 件を読み込みました",
                            saved.len()
                        );
                        engines = maintain::merge_defaults(saved);
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("aruaru-search: {e:#}"),
                }
                store = Some(Arc::new(s));
            }
            Err(e) => eprintln!("aruaru-search: GitHub の保存先を使いません({e:#})"),
        }
    }
    let searcher = Searcher::new(engines).expect("HTTP クライアントを作れません");
    if let Ok(mut w) = searcher.store.write() {
        *w = store;
    }
    // 意味による並べ替え(aruaru-llm の多言語の埋め込み)。`ARUARU_SEARCH_RERANK=off` で無効にできる
    let llm_base =
        std::env::var("ARUARU_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:4600".into());
    if std::env::var("ARUARU_SEARCH_RERANK").map_or(true, |v| v != "off") {
        searcher.set_rerank(Some(llm_base.clone()));
    }
    let ctx = Arc::new(Ctx {
        searcher,
        http: reqwest::Client::new(),
        llm_base,
        dir,
        admin_token: std::env::var("ARUARU_SEARCH_ADMIN_TOKEN").ok(),
        limiter: limit::Limiter::from_env(),
    });
    tokio::spawn(schedule(ctx.clone()));
    let (addr, handle) = Server::new(TcpListener::bind(bind)).run(app(ctx)).await?;
    println!("aruaru-search: http://{addr}/ で待受中(GET /v1/search?q=...)");
    tokio::select! {
        _ = handle => {}
        _ = tokio::signal::ctrl_c() => println!("aruaru-search: 終了します"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_string_decoding() {
        assert_eq!(
            query_param("q=%E5%B1%B1%E6%A2%A8+%E6%B8%A9%E6%B3%89&hl=ja", "q").as_deref(),
            Some("山梨 温泉")
        );
        assert_eq!(query_param("q=a&hl=ja", "hl").as_deref(), Some("ja"));
        assert_eq!(query_param("q=a", "zz"), None);
        assert_eq!(pct_decode("100%"), "100%");
    }

    #[test]
    fn jst_day_boundary_is_at_9_utc() {
        let (d1, h1) = jst_day_hour(86_400 * 10 + 8 * 3600); // UTC 08:00 → JST 17:00
        assert_eq!(h1, 17);
        let (d2, h2) = jst_day_hour(86_400 * 10 + 15 * 3600); // UTC 15:00 → JST 翌日 0:00
        assert_eq!((d2, h2), (d1 + 1, 0));
    }
}
