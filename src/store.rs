//! 検索元の設定と保守履歴の保存先: GitHub の非公開リポジトリ(VPS の DB・ディスクには保存しない)。
//!
//! 環境変数 `ARUARU_SEARCH_ARCHIVE_REPO`(例: `https://github.com/aon-co-jp/realdata-archive.git`)で指定する。
//! - `aruaru-search/engines.json` に、AI が直したものを含む検索元の設定を保存する。上書きのたびに1コミット。
//!   `git log` がそのまま設定の版の履歴になり、VPS を作り直しても直した設定を取り戻せる。
//! - `aruaru-search/maintenance/<年>/<月>/<UNIX時刻>-<検索元>.json` に、保守の経緯(直した・断った等)を1件1ファイルで残す。
//!
//! 認証は VPS の git の設定に任せる(トークンはこのプログラムでは扱わない)。仕組みは [`crate::archive`] を参照。

use anyhow::Result;

use crate::archive;
use crate::engine::EngineDef;

const ENGINES_PATH: &str = "aruaru-search/engines.json";

pub struct Store {
    repo: String,
}

/// ファイル名に使える形にする
fn file_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect()
}

/// UNIX 時刻 → (年, 月)。UTC の暦(閏年を含む)。
fn year_month(unix: u64) -> (u64, u64) {
    let days = unix / 86_400;
    // 1970-01-01 からの日数を年月日に直す(民間で使われる標準的な方法)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as u64, m as u64)
}

impl Store {
    pub async fn connect(repo: &str) -> Result<Store> {
        archive::check(repo).await?;
        Ok(Store {
            repo: repo.to_string(),
        })
    }

    /// 保存済みの検索元の設定(無ければ空)。
    pub async fn load_engines(&self) -> Result<Vec<EngineDef>> {
        let got = archive::read_many(&self.repo, "HEAD", &[ENGINES_PATH.to_string()]).await?;
        let Some(bytes) = got.into_iter().next().and_then(|(_, b)| b) else {
            return Ok(Vec::new());
        };
        let v: Vec<EngineDef> = serde_json::from_slice(&bytes).unwrap_or_default();
        Ok(v.into_iter().filter(|e| e.validate().is_ok()).collect())
    }

    /// 検索元の設定を保存する(1コミット)。コミット ID を返す。
    pub async fn save_engines(
        &self,
        engines: &[EngineDef],
        _now_unix: u64,
        message: &str,
    ) -> Result<String> {
        let json = serde_json::to_vec_pretty(engines)?;
        let pushed =
            archive::push(&self.repo, &[(ENGINES_PATH.to_string(), json)], message).await?;
        Ok(pushed.commit)
    }

    /// 保守の経緯を1件1ファイルで残す。
    pub async fn log(
        &self,
        now_unix: u64,
        engine: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<()> {
        let (y, m) = year_month(now_unix);
        let path = format!(
            "aruaru-search/maintenance/{y:04}/{m:02}/{now_unix}-{}.json",
            file_safe(engine)
        );
        let body = serde_json::to_vec_pretty(&serde_json::json!({
            "unix": now_unix, "engine": engine, "outcome": outcome, "detail": detail
        }))?;
        archive::push(
            &self.repo,
            &[(path, body)],
            &format!("maintenance {outcome} {}", file_safe(engine)),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_file_names() {
        assert_eq!(year_month(0), (1970, 1));
        assert_eq!(year_month(951_782_400), (2000, 2)); // 2000-02-29(閏日)
        assert_eq!(year_month(1_790_299_944), (2026, 9));
        assert_eq!(file_safe("../a b"), "___a_b");
    }
}
