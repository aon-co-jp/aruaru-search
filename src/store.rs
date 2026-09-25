//! aruaru-db による設定と保守履歴の保存・版管理(任意)。
//!
//! 環境変数 `ARUARU_SEARCH_DB_DSN`(例: `host=127.0.0.1 port=5433 user=... password=... dbname=aruaru`)を
//! 設定したときだけ使う。未設定なら、これまでどおりファイル(`engines.json`・`maintenance.log`)だけで動く。
//! - 検索元の設定(セレクタ)は `search_engine_configs` に保存し、AI が直すたびに `aruaru_commit` で版を残す。
//!   VPS の作り直しやディスクの入れ替えがあっても、直した設定を取り戻せる。
//! - 保守の経緯は `search_maintenance` に残す。
//!
//! aruaru-db の INSERT の解析は値をカンマで単純に区切るため(2026-09-24 に発見)、文字列は Base64 にして保存する。

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use tokio_postgres::{Client, NoTls};

use crate::engine::EngineDef;

fn enc(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

fn dec(s: &str) -> Result<String> {
    let b = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| anyhow!("保存された値を復号できません: {e}"))?;
    String::from_utf8(b).map_err(|e| anyhow!("保存された値が UTF-8 ではありません: {e}"))
}

pub struct Store {
    client: Client,
}

impl Store {
    pub async fn connect(dsn: &str) -> Result<Store> {
        let (client, conn) = tokio_postgres::connect(dsn, NoTls)
            .await
            .map_err(|e| anyhow!("aruaru-db に接続できません: {e}"))?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                eprintln!("aruaru-search: aruaru-db との接続が切れました: {e}");
            }
        });
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS search_engine_configs (id TEXT PRIMARY KEY, json TEXT NOT NULL, updated_unix TEXT NOT NULL);\
                 CREATE TABLE IF NOT EXISTS search_maintenance (unix TEXT NOT NULL, engine TEXT NOT NULL, outcome TEXT NOT NULL, detail TEXT NOT NULL);",
            )
            .await
            .context("テーブルを準備できません")?;
        Ok(Store { client })
    }

    /// 保存済みの検索元の設定(無ければ空)。
    pub async fn load_engines(&self) -> Result<Vec<EngineDef>> {
        let rows = self
            .client
            .query("SELECT id, json FROM search_engine_configs", &[])
            .await
            .context("設定の読み込みに失敗")?;
        let mut v = Vec::new();
        for r in rows {
            let json = dec(&r.get::<_, String>(1))?;
            if let Ok(e) = serde_json::from_str::<EngineDef>(&json) {
                if e.validate().is_ok() {
                    v.push(e);
                }
            }
        }
        Ok(v)
    }

    /// 検索元の設定を保存し、その時点を版として記録する。コミット ID を返す。
    pub async fn save_engines(
        &self,
        engines: &[EngineDef],
        now_unix: u64,
        message: &str,
    ) -> Result<String> {
        for e in engines {
            self.client
                .execute("DELETE FROM search_engine_configs WHERE id = $1", &[&e.id])
                .await
                .context("設定の保存(削除)に失敗")?;
            self.client
                .execute(
                    "INSERT INTO search_engine_configs (id, json, updated_unix) VALUES ($1, $2, $3)",
                    &[&e.id, &enc(&serde_json::to_string(e)?), &now_unix.to_string()],
                )
                .await
                .context("設定の保存(挿入)に失敗")?;
        }
        let row = self
            .client
            .query_opt("SELECT aruaru_commit($1)", &[&message])
            .await
            .context("版の記録に失敗")?
            .ok_or_else(|| anyhow!("aruaru_commit がコミット ID を返しませんでした"))?;
        row.try_get::<_, String>(0)
            .map_err(|_| anyhow!("コミット ID を読めません"))
    }

    pub async fn log(
        &self,
        now_unix: u64,
        engine: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<()> {
        self.client
            .execute(
                "INSERT INTO search_maintenance (unix, engine, outcome, detail) VALUES ($1, $2, $3, $4)",
                &[&now_unix.to_string(), &enc(engine), &enc(outcome), &enc(detail)],
            )
            .await
            .context("保守の履歴を保存できません")?;
        Ok(())
    }
}
