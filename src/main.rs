//! aruaru-search: Rust 製の自前メタ検索。APIキー不要・VPS で完全無料。
//!
//! RPoem(`open-runo-poem-compat`)の上で JSON の検索 API を提供する。
//! Connect(`POST /aruaru_search.<Service>/<Method>`、JSON)で提供する。詳細は `connect.rs`。
//! - `Search/Search`・`Search/MediaSearch`・`Search/Engines`・`Search/Health`・`Admin/SelfCheck`
//! - `GET /healthz`
//!
//! 毎朝7時(日本時間)と起動時に、全ての検索元を点検し、読み取れなくなったものは AI(aruaru-llm)で自動保守する。

mod archive;
mod connect;
mod archive_org;
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

use open_runo_poem_compat::hyper_compat::{html_response, json_response};
use open_runo_poem_compat::{
    get, handler_fn, Request, Route, Server, StatusCode, TcpListener,
};
use serde_json::json;

use search::Searcher;

const INDEX_HTML: &str = include_str!("../web_index.html");

pub struct Ctx {
    pub searcher: Arc<Searcher>,
    pub http: reqwest::Client,
    pub llm_base: String,
    pub dir: PathBuf,
    pub admin_token: Option<String>,
    pub limiter: limit::Limiter,
}

pub fn client_of(req: &Request) -> Option<String> {
    let h = |k: &str| req.headers().get(k).and_then(|v| v.to_str().ok());
    limit::client_key(h("x-forwarded-for"), h("x-real-ip"))
}

fn app(ctx: Arc<Ctx>) -> Route {
    let connect_ctx = ctx.clone();
    let route = Route::new()
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
        );
    connect::routes(route, connect_ctx)
        .docs_auto("/api", "aruaru-search", env!("CARGO_PKG_VERSION"))
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
    // 休止中の検索元を、時刻が来たら探って自動で復帰させる(成功なら即復帰、失敗なら指数バックオフ)
    let s2 = ctx.searcher.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            s2.probe_due(None).await;
        }
    });
    let (addr, handle) = Server::new(TcpListener::bind(bind)).run(app(ctx)).await?;
    println!("aruaru-search: http://{addr}/ で待受中(Connect: POST /aruaru_search.Search/Search)");
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
    fn jst_day_boundary_is_at_9_utc() {
        let (d1, h1) = jst_day_hour(86_400 * 10 + 8 * 3600); // UTC 08:00 → JST 17:00
        assert_eq!(h1, 17);
        let (d2, h2) = jst_day_hour(86_400 * 10 + 15 * 3600); // UTC 15:00 → JST 翌日 0:00
        assert_eq!((d2, h2), (d1 + 1, 0));
    }
}
