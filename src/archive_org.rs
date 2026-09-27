//! archive.org(Internet Archive)を、曲名・演奏者・作曲者・レーベル名で横断検索し、
//! ライセンス上問題のない(パブリックドメイン・CC0・CC-BY(-SA))音源だけを、即時ストリーミング
//! 可能な形で返す。
//!
//! 既存の `engine.rs`(検索エンジンの公開結果ページを HTML として解析する仕組み)とは別枠とする:
//! archive.org は構造化された JSON の検索 API(`advancedsearch.php`)とメタデータ API
//! (`metadata/<identifier>`)を持つ、通常の Web 検索エンジンとは性質の異なるデータ源であるため。
//! `archive.rs`(GitHub 非公開リポジトリへの保存)と同様、専用モジュールとして切り出す。

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const SEARCH_URL: &str = "https://archive.org/advancedsearch.php";
const METADATA_BASE: &str = "https://archive.org/metadata";
const DOWNLOAD_BASE: &str = "https://archive.org/download";
const DETAILS_BASE: &str = "https://archive.org/details";
/// 検索・メタデータ取得それぞれの待ち時間の上限
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MediaHit {
    pub identifier: String,
    pub title: String,
    /// 演奏者・作曲者(archive.org の `creator`。複数のことがあるため、`; ` で連結)
    pub creator: String,
    pub date: String,
    pub licenseurl: String,
    /// 詳細ページ(人が見るページ)
    pub item_url: String,
    /// 即時ストリーミング可能な音声ファイルの直接 URL(見つからなければ None)
    pub stream_url: Option<String>,
}

#[derive(Deserialize)]
struct SearchResponse {
    response: SearchResponseInner,
}

#[derive(Deserialize)]
struct SearchResponseInner {
    docs: Vec<SearchDoc>,
}

/// archive.org の `fl[]` で `creator` を複数返す場合は配列になるため、文字列・配列の両方を受け付ける。
#[derive(Deserialize, Default)]
struct SearchDoc {
    identifier: String,
    #[serde(default)]
    title: StringOrList,
    #[serde(default)]
    creator: StringOrList,
    #[serde(default)]
    date: StringOrList,
    #[serde(default)]
    licenseurl: StringOrList,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum StringOrList {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn joined(&self, sep: &str) -> String {
        match self {
            StringOrList::None => String::new(),
            StringOrList::One(s) => s.clone(),
            StringOrList::Many(v) => v.join(sep),
        }
    }
    fn first(&self) -> String {
        match self {
            StringOrList::None => String::new(),
            StringOrList::One(s) => s.clone(),
            StringOrList::Many(v) => v.first().cloned().unwrap_or_default(),
        }
    }
}

/// このライセンス URL の音源を、非商用限定などの制限なしに配信してよいか。
/// 許可: パブリックドメイン(publicdomain/zero・publicdomain/mark 等)・CC0・CC-BY・CC-BY-SA。
/// 拒否: CC-BY-NC・CC-BY-ND・CC-BY-NC-SA・CC-BY-NC-ND、その他不明なもの。
pub fn is_allowed_license(licenseurl: &str) -> bool {
    let u = licenseurl.trim().to_ascii_lowercase();
    if u.is_empty() {
        return false;
    }
    if !u.contains("creativecommons.org") && !u.contains("publicdomain") {
        return false;
    }
    if u.contains("/publicdomain/") {
        // publicdomain/zero(CC0)・publicdomain/mark はどちらも制限なし
        return true;
    }
    // .../licenses/<種別>/<版>/ の <種別> だけを見る(nc・nd を含む種別は拒否)
    let Some(after) = u.split("/licenses/").nth(1) else {
        return false;
    };
    let kind = after.split('/').next().unwrap_or("");
    matches!(kind, "by" | "by-sa")
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent("aruaru-search/1.0 (+https://github.com/aon-co-jp/aruaru-search)")
        .build()
        .context("HTTP クライアントを作れません")
}

/// 曲名・演奏者・作曲者・レーベル名などで archive.org を検索し、許可されたライセンスの音源だけを返す。
/// `mediatype` は既定 `audio`(動画も含めたければ呼び出し側で変える余地を残すが、現状は音声専用)。
pub async fn search(q: &str, n: usize) -> Result<Vec<MediaHit>> {
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 300 {
        bail!("検索語は1〜300文字にしてください");
    }
    let n = n.clamp(1, 50);
    let http = client()?;
    // ライセンス種別は archive.org 側のフィールド検索では絞り込みが不安定なため、広めに取得してから
    // `is_allowed_license` でこちら側で厳密に絞る(取りこぼしより誤って NC 音源を通すほうを避ける)。
    let query = format!("mediatype:(audio) AND ({})", escape_query(q));
    let resp = http
        .get(SEARCH_URL)
        .query(&[
            ("q", query.as_str()),
            ("fl[]", "identifier"),
            ("fl[]", "title"),
            ("fl[]", "creator"),
            ("fl[]", "date"),
            ("fl[]", "licenseurl"),
            ("rows", &(n * 4).min(200).to_string()),
            ("output", "json"),
        ])
        .send()
        .await
        .context("archive.org へ接続できません")?;
    if !resp.status().is_success() {
        bail!("archive.org が拒否しました(HTTP {})", resp.status());
    }
    let body: SearchResponse = resp
        .json()
        .await
        .context("archive.org の応答を読み取れません")?;
    let mut hits: Vec<MediaHit> = Vec::new();
    for d in body.response.docs {
        if d.identifier.is_empty() {
            continue;
        }
        let licenseurl = d.licenseurl.first();
        if !is_allowed_license(&licenseurl) {
            continue;
        }
        hits.push(MediaHit {
            item_url: format!("{DETAILS_BASE}/{}", d.identifier),
            title: d.title.first(),
            creator: d.creator.joined("; "),
            date: d.date.first(),
            licenseurl,
            identifier: d.identifier,
            stream_url: None,
        });
        if hits.len() >= n {
            break;
        }
    }
    // 上位だけ、実際に再生できるファイルの直接 URL を埋める(全件だと重いため)
    for h in hits.iter_mut() {
        h.stream_url = stream_url_for(&http, &h.identifier).await.ok().flatten();
    }
    Ok(hits)
}

fn escape_query(q: &str) -> String {
    // archive.org の検索構文で特別な意味を持つ引用符だけ落とす(それ以外はそのまま渡す)
    q.replace('"', "")
}

#[derive(Deserialize)]
struct MetadataResponse {
    #[serde(default)]
    files: Vec<MetadataFile>,
}

#[derive(Deserialize)]
struct MetadataFile {
    name: String,
    #[serde(default)]
    format: String,
}

/// この識別子の中から、そのまま再生できる音声ファイル(mp3 を優先)の直接ダウンロード URL を探す。
pub async fn stream_url_for(http: &reqwest::Client, identifier: &str) -> Result<Option<String>> {
    let resp = http
        .get(format!("{METADATA_BASE}/{identifier}"))
        .send()
        .await
        .context("archive.org のメタデータへ接続できません")?;
    if !resp.status().is_success() {
        bail!("メタデータの取得に失敗しました(HTTP {})", resp.status());
    }
    let meta: MetadataResponse = resp
        .json()
        .await
        .context("archive.org のメタデータを読み取れません")?;
    let pick = |fmt_contains: &str| {
        meta.files
            .iter()
            .find(|f| f.format.to_ascii_lowercase().contains(fmt_contains))
    };
    let file = pick("mp3")
        .or_else(|| pick("ogg vorbis"))
        .or_else(|| pick("flac"));
    Ok(file.map(|f| format!("{DOWNLOAD_BASE}/{identifier}/{}", f.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_public_domain_and_cc0_and_by_and_by_sa() {
        assert!(is_allowed_license(
            "https://creativecommons.org/publicdomain/zero/1.0/"
        ));
        assert!(is_allowed_license(
            "http://creativecommons.org/publicdomain/mark/1.0/"
        ));
        assert!(is_allowed_license(
            "https://creativecommons.org/licenses/by/4.0/"
        ));
        assert!(is_allowed_license(
            "https://creativecommons.org/licenses/by-sa/3.0/"
        ));
    }

    #[test]
    fn rejects_noncommercial_and_noderivatives_and_unknown() {
        assert!(!is_allowed_license(
            "https://creativecommons.org/licenses/by-nc/4.0/"
        ));
        assert!(!is_allowed_license(
            "https://creativecommons.org/licenses/by-nd/4.0/"
        ));
        assert!(!is_allowed_license(
            "https://creativecommons.org/licenses/by-nc-sa/4.0/"
        ));
        assert!(!is_allowed_license(
            "https://creativecommons.org/licenses/by-nc-nd/4.0/"
        ));
        assert!(!is_allowed_license(""));
        assert!(!is_allowed_license("https://example.com/all-rights-reserved"));
    }

    #[test]
    fn string_or_list_joins_and_takes_first() {
        let many = StringOrList::Many(vec!["A".into(), "B".into()]);
        assert_eq!(many.joined("; "), "A; B");
        assert_eq!(many.first(), "A");
        assert_eq!(StringOrList::None.first(), "");
    }

    #[test]
    fn parses_a_realistic_archive_org_search_response() {
        let json = r#"{
          "response": { "docs": [
            { "identifier": "good-song", "title": "Good Song", "creator": ["Jane Doe"],
              "date": "1925-01-01", "licenseurl": "https://creativecommons.org/publicdomain/zero/1.0/" },
            { "identifier": "nc-song", "title": "NC Song", "creator": "John Roe",
              "date": "2020-01-01", "licenseurl": "https://creativecommons.org/licenses/by-nc/4.0/" },
            { "identifier": "no-license" }
          ] }
        }"#;
        let parsed: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.response.docs.len(), 3);
        assert_eq!(parsed.response.docs[0].identifier, "good-song");
        assert!(is_allowed_license(&parsed.response.docs[0].licenseurl.first()));
        assert!(!is_allowed_license(&parsed.response.docs[1].licenseurl.first()));
        assert!(!is_allowed_license(&parsed.response.docs[2].licenseurl.first()));
    }
}
