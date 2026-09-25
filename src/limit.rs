//! 利用者ごとの回数制限(公開したときに、VPS の IP が検索元に拒否されるほど使われないようにする)。
//!
//! - 利用者は、プロキシ(open-web-server など)が付ける `X-Forwarded-For` / `X-Real-IP` の先頭の IP で見分ける。
//! - そのヘッダが**無い**呼び出し(VPS の中の aruaru-llm や realdata.pro)は、内部の利用として制限しない
//!   (`ARUARU_SEARCH_LOCAL_UNLIMITED=0` にすると、内部も制限する)。
//! - 既定は、1利用者あたり 1分に20回・1日に1,000回(環境変数 `ARUARU_SEARCH_RATE_PER_MIN` / `ARUARU_SEARCH_RATE_PER_DAY`)。
//!
//! 公開するときは、プロキシがクライアントの IP を必ず付け直すこと(利用者が自分でヘッダを付けて回数を偽れないように)。

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default, Clone, Copy)]
struct Bucket {
    minute: u64,
    minute_n: u32,
    day: u64,
    day_n: u32,
}

pub struct Limiter {
    per_min: u32,
    per_day: u32,
    local_unlimited: bool,
    buckets: Mutex<HashMap<String, Bucket>>,
}

/// 記憶しておく利用者の最大数(超えたら古い記録をすべて忘れる。攻撃でメモリを使い切られないため)
const MAX_CLIENTS: usize = 20_000;

impl Limiter {
    pub fn new(per_min: u32, per_day: u32, local_unlimited: bool) -> Limiter {
        Limiter {
            per_min,
            per_day,
            local_unlimited,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_env() -> Limiter {
        let n = |k: &str, d: u32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        Limiter::new(
            n("ARUARU_SEARCH_RATE_PER_MIN", 20),
            n("ARUARU_SEARCH_RATE_PER_DAY", 1_000),
            std::env::var("ARUARU_SEARCH_LOCAL_UNLIMITED").map_or(true, |v| v != "0"),
        )
    }

    /// 1回の利用を数える。制限を超えていれば、何秒後にやり直せるかを Err で返す。
    /// `client` が None なら内部の利用。
    pub fn check(&self, client: Option<&str>, now_unix: u64) -> Result<(), u64> {
        let Some(key) = client else {
            return if self.local_unlimited {
                Ok(())
            } else {
                self.count("local", now_unix)
            };
        };
        self.count(key, now_unix)
    }

    fn count(&self, key: &str, now: u64) -> Result<(), u64> {
        let mut m = self.buckets.lock().map_err(|_| 60u64)?;
        if m.len() >= MAX_CLIENTS && !m.contains_key(key) {
            m.clear();
        }
        let b = m.entry(key.to_string()).or_default();
        let (minute, day) = (now / 60, (now + 9 * 3600) / 86_400);
        if b.minute != minute {
            b.minute = minute;
            b.minute_n = 0;
        }
        if b.day != day {
            b.day = day;
            b.day_n = 0;
        }
        if b.minute_n >= self.per_min {
            return Err(60 - now % 60);
        }
        if b.day_n >= self.per_day {
            return Err(86_400 - (now + 9 * 3600) % 86_400);
        }
        b.minute_n += 1;
        b.day_n += 1;
        Ok(())
    }
}

/// プロキシが付けるヘッダから、利用者の IP を取り出す。ヘッダが無ければ None(内部の利用)。
/// ヘッダがあっても形が正しくなければ、まとめて "invalid" として(厳しく)数える。
pub fn client_key(x_forwarded_for: Option<&str>, x_real_ip: Option<&str>) -> Option<String> {
    let raw = x_forwarded_for
        .and_then(|v| v.split(',').next())
        .or(x_real_ip)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let valid = raw.len() <= 45
        && raw
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':');
    Some(if valid {
        raw.to_ascii_lowercase()
    } else {
        "invalid".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_minute_and_per_day_limits_apply_per_client() {
        let l = Limiter::new(3, 5, true);
        let t = 1_790_000_000;
        for _ in 0..3 {
            assert!(l.check(Some("1.2.3.4"), t).is_ok());
        }
        let wait = l.check(Some("1.2.3.4"), t).unwrap_err();
        assert!((1..=60).contains(&wait));
        // 別の利用者には影響しない
        assert!(l.check(Some("5.6.7.8"), t).is_ok());
        // 次の分になれば、また使える(1日の上限5回まで)
        assert!(l.check(Some("1.2.3.4"), t + 60).is_ok());
        assert!(l.check(Some("1.2.3.4"), t + 60).is_ok());
        assert!(l.check(Some("1.2.3.4"), t + 120).is_err(), "1日の上限");
        // 翌日(日本時間の日付が変わったあと)には戻る
        assert!(l.check(Some("1.2.3.4"), t + 86_400).is_ok());
    }

    #[test]
    fn internal_calls_are_unlimited_unless_configured() {
        let free = Limiter::new(1, 1, true);
        for _ in 0..50 {
            assert!(free.check(None, 100).is_ok());
        }
        let strict = Limiter::new(1, 1, false);
        assert!(strict.check(None, 100).is_ok());
        assert!(strict.check(None, 100).is_err());
    }

    #[test]
    fn client_keys_come_from_proxy_headers_and_are_validated() {
        assert_eq!(client_key(None, None), None);
        assert_eq!(
            client_key(Some("203.0.113.9, 10.0.0.1"), None).as_deref(),
            Some("203.0.113.9")
        );
        assert_eq!(
            client_key(None, Some("2001:DB8::1")).as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(
            client_key(Some("<script>"), None).as_deref(),
            Some("invalid")
        );
        assert_eq!(client_key(Some("  "), None), None);
    }

    #[test]
    fn memory_is_bounded() {
        let l = Limiter::new(1, 1, true);
        for i in 0..(MAX_CLIENTS + 10) {
            let _ = l.check(Some(&format!("c{i}")), 100);
        }
        assert!(l.buckets.lock().unwrap().len() <= MAX_CLIENTS);
    }
}
