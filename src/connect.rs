//! Connect の「単純呼び出し(unary)」で受ける API(2026-11-04、REST から移行)。
//!
//! 形式は `POST /aruaru_search.<Service>/<Method>`、本文は JSON(`content-type: application/json`)。
//! 読み込みは RS-JSON の `from_str_secure`(サイズ・深さ・重複キーの上限つき)、書き出しは `rust_json::fast`。
//! エラーは Connect の形式 `{"code": "...", "message": "..."}`。
//!
//! - `Search/Search`      `{q, hl, gl, n}` → `{results, warnings}`
//! - `Search/MediaSearch` `{q, n}` → `{results}`
//! - `Search/Engines`     `{}` → `{engines, status}`
//! - `Search/Health`      `{}` → 検索元ごとの状態
//! - `Admin/SelfCheck`    `{}` + `x-admin-token` → `{report}`

use std::sync::Arc;

use http_body_util::{BodyExt, Limited};
use open_runo_poem_compat::hyper_compat::json_response;
use open_runo_poem_compat::{handler_fn, post, Request, Response, Route, StatusCode};
use rust_json::secure::Limits;
use serde::{Deserialize, Serialize};

use crate::{archive_org, client_of, maintain, search, Ctx};

const BODY_MAX: usize = 64 << 10;

fn status_of(code: &str) -> StatusCode {
    match code {
        "invalid_argument" => StatusCode::BAD_REQUEST,
        "permission_denied" => StatusCode::FORBIDDEN,
        "resource_exhausted" => StatusCode::TOO_MANY_REQUESTS,
        "unavailable" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn fail(code: &str, message: &str) -> Response {
    json_response(
        status_of(code),
        &serde_json::json!({ "code": code, "message": message }),
    )
}

fn ok(value: &impl Serialize) -> Response {
    // RS-JSON の `to_vec` が書き出したバイト列をそのまま返す(以前は serde_json::Value に
    // 読み戻して書き直していた二重の変換をやめた)
    match rust_json::fast::to_vec(value) {
        Ok(b) => {
            let mut resp = Response::new(open_runo_poem_compat::hyper_compat::fixed_body(
                bytes::Bytes::from(b),
            ));
            if let Ok(v) = "application/json; charset=utf-8".parse() {
                resp.headers_mut().insert("content-type", v);
            }
            resp
        }
        Err(_) => fail("internal", "serialize failed"),
    }
}

/// 本文を RS-JSON の上限つきで読む。空の本文は既定値として扱う。
async fn read<T: serde::de::DeserializeOwned + Default>(req: Request) -> Result<T, Response> {
    let bytes = Limited::new(req.into_body(), BODY_MAX)
        .collect()
        .await
        .map_err(|_| fail("resource_exhausted", "request body too large / 本文が大きすぎます"))?
        .to_bytes();
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    let s = std::str::from_utf8(&bytes)
        .map_err(|_| fail("invalid_argument", "body must be UTF-8 / 本文は UTF-8 にしてください"))?;
    rust_json::secure::from_str_secure(
        s,
        &Limits { max_bytes: BODY_MAX, ..Limits::default() },
    )
    .map_err(|e| fail("invalid_argument", &format!("JSON が正しくありません: {e}")))
}

#[derive(Default, Deserialize)]
struct SearchIn {
    #[serde(default, alias = "query")]
    q: String,
    #[serde(default)]
    hl: String,
    #[serde(default)]
    gl: String,
    #[serde(default, alias = "max_results")]
    n: Option<usize>,
}

#[derive(Default, Deserialize)]
struct MediaIn {
    #[serde(default)]
    q: String,
    #[serde(default)]
    n: Option<usize>,
}

fn limited(ctx: &Ctx, req: &Request) -> Option<Response> {
    let client = client_of(req);
    ctx.limiter
        .check(client.as_deref(), search::now_unix())
        .err()
        .map(|wait| {
            fail(
                "resource_exhausted",
                &format!("利用回数の上限に達しました。{wait}秒後にもう一度お試しください"),
            )
        })
}

pub fn routes(route: Route, ctx: Arc<Ctx>) -> Route {
    let (c1, c2, c3, c4, c5) = (ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone(), ctx);
    route
        .at(
            "/aruaru_search.Search/Search",
            post(handler_fn(move |req: Request, _p| {
                let ctx = c1.clone();
                async move {
                    if let Some(r) = limited(&ctx, &req) {
                        return r;
                    }
                    let i: SearchIn = match read(req).await {
                        Ok(i) => i,
                        Err(r) => return r,
                    };
                    let ok_tag = |s: &str, max: usize| {
                        s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    };
                    if !ok_tag(&i.hl, 12) || !ok_tag(&i.gl, 6) {
                        return fail("invalid_argument", "hl / gl が正しくありません");
                    }
                    match ctx.searcher.search(&i.q, &i.hl, &i.gl, i.n.unwrap_or(10)).await {
                        Ok((results, warnings)) => {
                            ok(&serde_json::json!({ "results": results, "warnings": warnings }))
                        }
                        Err(e) => fail("unavailable", &format!("{e:#}")),
                    }
                }
            })),
        )
        .at(
            "/aruaru_search.Search/MediaSearch",
            post(handler_fn(move |req: Request, _p| {
                let ctx = c2.clone();
                async move {
                    if let Some(r) = limited(&ctx, &req) {
                        return r;
                    }
                    let i: MediaIn = match read(req).await {
                        Ok(i) => i,
                        Err(r) => return r,
                    };
                    match archive_org::search_relaxed(&i.q, i.n.unwrap_or(10)).await {
                        Ok(hits) => ok(&serde_json::json!({ "results": hits })),
                        Err(e) => fail("unavailable", &format!("{e:#}")),
                    }
                }
            })),
        )
        .at(
            "/aruaru_search.Search/Engines",
            post(handler_fn(move |_req: Request, _p| {
                let ctx = c3.clone();
                async move {
                    let engines = ctx.searcher.engines.read().map(|e| e.clone()).unwrap_or_default();
                    let status = ctx.searcher.status.read().map(|s| s.clone()).unwrap_or_default();
                    ok(&serde_json::json!({ "engines": engines, "status": status }))
                }
            })),
        )
        .at(
            "/aruaru_search.Search/Health",
            post(handler_fn(move |_req: Request, _p| {
                let ctx = c4.clone();
                async move { ok(&ctx.searcher.health()) }
            })),
        )
        .at(
            "/aruaru_search.Admin/SelfCheck",
            post(handler_fn(move |req: Request, _p| {
                let ctx = c5.clone();
                async move {
                    let given = req
                        .headers()
                        .get("x-admin-token")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    match &ctx.admin_token {
                        Some(t) if !t.is_empty() && t == given => {}
                        _ => return fail("permission_denied", "管理トークンが必要です"),
                    }
                    let report =
                        maintain::selfcheck(&ctx.searcher, &ctx.http, &ctx.llm_base, &ctx.dir).await;
                    ok(&serde_json::json!({ "report": report }))
                }
            })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ok_writes_json_bytes_directly_with_json_content_type() {
        let resp = ok(&serde_json::json!({ "results": [{"title": "東京"}], "warnings": [] }));
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
            Some("application/json; charset=utf-8")
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["results"][0]["title"], "東京");
    }

    #[tokio::test]
    async fn fail_maps_codes_to_http_status() {
        assert_eq!(fail("invalid_argument", "x").status(), StatusCode::BAD_REQUEST);
        assert_eq!(fail("resource_exhausted", "x").status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(fail("unavailable", "x").status(), StatusCode::BAD_GATEWAY);
    }
}
