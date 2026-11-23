# aruaru-search 方針 / Policy

## 日本語
- **API**: REST は全廃済み。呼び出しは Connect の単純呼び出し(`POST /aruaru_search.v1.Search/{Search,MediaSearch,Engines,Health}`、`POST /aruaru_search.v1.Admin/SelfCheck`、本文は JSON)。例外として `GET /healthz` だけが残る(ヘルスチェック)。正本は `runo/API-POLICY.md`。
- **JSON**: RS-JSON に統一。外から来る JSON(archive.org・検索元 API・aruaru-llm の応答・HTTP 本文)は `from_str_secure`(サイズ・深さ・重複キーの上限つき)で読み、応答は `rust_json::fast` で書き出したバイト列をそのまま返す。
- **検索元**: 違いはコードではなく設定(`engines.default.json` / `data/engines.json`)。AI が直せるのはセレクタと転送パラメータ名だけ。URL・重みは変えさせない。AI が連続で「直せない」と判断したときだけ自動で無効化し、定期の点検で回復を試す。設定の `rev` を上げると保存済み設定にも反映される。
- **負担をかけない**: 間隔・拒否後の休止・キャッシュを弱めない。休止中は固定の無害な語で探り、確認画面は突破しない。`robots.txt` で禁止されている検索元・ボット検知される検索元は使わない。
- **配備**: VPS では重いビルドをしない。WSL でビルドし、glibc 2.39 以下を `objdump -T` で確認して `scp`。旧バイナリを `.before-…` に退避し、入れ替えの前後で検索の成功を確認し、失敗したら戻す。
- **秘密**: トークン・パスワードを表示・コミットしない。

## English
- **API**: REST is abolished. Calls are Connect unary calls (`POST /aruaru_search.v1.Search/{Search,MediaSearch,Engines,Health}`, `POST /aruaru_search.v1.Admin/SelfCheck`, JSON body). The only exception is `GET /healthz` (health check). The source of truth is `runo/API-POLICY.md`.
- **JSON**: unified on RS-JSON. JSON from outside (archive.org, engine APIs, aruaru-llm responses, HTTP bodies) is read with `from_str_secure` (size, depth and duplicate-key limits); responses return the bytes written by `rust_json::fast` as they are.
- **Engines**: differences live in configuration, not code (`engines.default.json` / `data/engines.json`). The AI may change only selectors and forwarded parameter names, never URLs or weights. An engine is disabled automatically only after consecutive "cannot fix" judgements, and periodic checks try to bring it back. Raising an engine's `rev` propagates the change to saved configs.
- **Be gentle on sources**: keep intervals, post-refusal cooldowns and caching. Cooled-down engines are probed with a fixed harmless query; challenge pages are never bypassed. Engines forbidden by `robots.txt` or bot-detected are not used.
- **Deployment**: no heavy builds on the VPS. Build in WSL, check glibc ≤ 2.39 with `objdump -T`, then `scp`. Keep the old binary as `.before-…`, verify a real search before and after the swap, and roll back on failure.
- **Secrets**: never print or commit tokens or passwords.

## 追記(2026-11-24) / Addendum
- **日本語**: 呼び出し元を増やすときも REST は使わない。旧 REST を呼んでいる所を見つけたら、その場で Connect へ移す。確認は「VPS の実測(旧パスが404か)」と「`cargo test`」で行う。
- **English**: Never use REST when adding callers. If a caller of an old REST route is found, move it to Connect right away. Verify by measuring on the VPS (old paths return 404) and by `cargo test`.

## 2026-11-24 MediaSearch の探し直し / MediaSearch relaxation
**日本語**: `Search/MediaSearch` は、0件のときだけ助詞・冠詞を除き後ろの語から減らして最大3回探し直す(`archive_org::search_relaxed`、試験 51 件成功、VPS 反映済み)。拡張機能 v0.1.1 の既定の検索語 `lo-fi piano` は実測で 3 件(全て再生可能)。
**English**: When `Search/MediaSearch` finds nothing, it retries up to 3 times with stop words removed and trailing words dropped (`archive_org::search_relaxed`; 51 tests pass; deployed). The extension v0.1.1's default prompt `lo-fi piano` measured 3 playable hits.
