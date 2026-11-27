# aruaru-search 方針 / Policy

## 日本語
- **API**: REST は全廃済み。呼び出しは Connect の単純呼び出し(`POST /aruaru_search.Search/{Search,MediaSearch,Engines,Health}`、`POST /aruaru_search.Admin/SelfCheck`、本文は JSON)。例外として `GET /healthz` だけが残る(ヘルスチェック)。正本は `runo/API-POLICY.md`。
- **JSON**: RS-JSON に統一。外から来る JSON(archive.org・検索元 API・aruaru-llm の応答・HTTP 本文)は `from_str_secure`(サイズ・深さ・重複キーの上限つき)で読み、応答は `rust_json::fast` で書き出したバイト列をそのまま返す。
- **検索元**: 違いはコードではなく設定(`engines.default.json` / `data/engines.json`)。AI が直せるのはセレクタと転送パラメータ名だけ。URL・重みは変えさせない。AI が連続で「直せない」と判断したときだけ自動で無効化し、定期の点検で回復を試す。設定の `rev` を上げると保存済み設定にも反映される。
- **負担をかけない**: 間隔・拒否後の休止・キャッシュを弱めない。休止中は固定の無害な語で探り、確認画面は突破しない。`robots.txt` で禁止されている検索元・ボット検知される検索元は使わない。
- **配備**: VPS では重いビルドをしない。WSL でビルドし、glibc 2.39 以下を `objdump -T` で確認して `scp`。旧バイナリを `.before-…` に退避し、入れ替えの前後で検索の成功を確認し、失敗したら戻す。
- **秘密**: トークン・パスワードを表示・コミットしない。

## English
- **API**: REST is abolished. Calls are Connect unary calls (`POST /aruaru_search.Search/{Search,MediaSearch,Engines,Health}`, `POST /aruaru_search.Admin/SelfCheck`, JSON body). The only exception is `GET /healthz` (health check). The source of truth is `runo/API-POLICY.md`.
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

## Versionless API の方針 / Versionless API policy (2026-11-25)
**日本語**: このリポジトリのAPIは **Versionless**(URL・パッケージ名に `v1` 等を付けず、互換性を壊さず足すだけで進化させる)。欄は足すだけ・足す欄は省略可能・知らない欄は無視・名前変更は新名を足して旧を非推奨に(期限は既定で6か月以上、使用0を確認してから削除)・enumは未知の値を許容・安全上の緊急修正は例外。既存の `.v1` は呼び出し側を壊さないよう当面残し、版なしの別名を追加して段階的に畳む。詳細・出典は [`runo/API-POLICY.md`](https://github.com/aon-co-jp/runo/blob/main/API-POLICY.md) 末尾。適用はこれから順に行う。
**English**: This repository's APIs are **Versionless**: no `v1` in URLs or package names; evolve by adding only, without breaking compatibility. Only add fields, new fields are optional, ignore unknown fields, rename by adding the new name and deprecating the old (default deadline 6 months or more, remove only after confirming zero use), tolerate unknown enum values, emergency security fixes are the exception. Existing `.v1` names stay for now so callers do not break; a versionless alias is added and `.v1` is retired in stages. Details and sources: the end of [`runo/API-POLICY.md`](https://github.com/aon-co-jp/runo/blob/main/API-POLICY.md). Rollout is step by step.

## API 方針: REST 全廃・gRPC / Connect へ
**日本語**: このリポジトリでは **REST API を新規に作らない**。ブラウザ・`curl` から呼ぶ API は **Connect の単純呼び出し**(`POST /<パッケージ>.<Service>/<Method>`、`content-type: application/json`)、サービス間は **gRPC / gRPC-web**。GraphQL は読み取り専用。既存の REST は触るときに移し、移したら旧経路は消す。JSON は RS-JSON(`from_str_secure` / `parse_secure`)で読む。例外は相手の規格が形を決めるもの(ACME、OAuth/OIDC のコールバック、第三者の Webhook、ヘルスチェック、静的ファイル、OTLP)のみ。迷ったら Connect。正本は [`runo/API-POLICY.md`](https://github.com/aon-co-jp/runo/blob/main/API-POLICY.md)。
**English**: This repository **does not add new REST APIs**. APIs called from browsers or `curl` use **Connect unary calls** (`POST /<package>.<Service>/<Method>`, `content-type: application/json`); service-to-service calls use **gRPC / gRPC-web**. GraphQL is read-only. Existing REST is migrated when touched and the old route is removed. Read JSON with RS-JSON (`from_str_secure` / `parse_secure`). Exceptions are only shapes dictated by someone else's standard (ACME, OAuth/OIDC callbacks, third-party webhooks, health checks, static files, OTLP). When in doubt, use Connect. Source of truth: the link above.

## 完成まで作り切る方針 / Finish everything policy (2026-11-27)
**日本語**: 全リポジトリで、**未確認は自動で確認し、未実施は実施し、未了は完成させる**(確認を求めず進める)。使用頻度が低いと判断される機能や、**1から作り直す大規模開発になる内容でも、やめずに最後まで開発・TEST・DEBUGする**(別の方式で済ませない、後回しにしない)。進め方: 調査(英語・日本語の検索とGitHub) → 設計 → 実装 → 実機/実データで確認 → 失敗は原因を直して再試験 → README・CLAUDE・PORTING・POLICY を日英で更新 → push。「できた」と書くのは実測した範囲だけ、確認できなかった所は「未確認」と正直に書く。例外(確認を取る): 元に戻せない削除、秘密の公開、所有者の認証・決済・実機操作が要るもの、利用者のシステム設定の変更。
**English**: In every repository, **verify what is unverified, do what is not yet done, and finish what is unfinished** automatically, without asking. Even features that look rarely used, or work that means **a large rewrite from scratch, are developed, tested and debugged to the end** (no substituting another approach, no postponing). Method: research (English and Japanese search plus GitHub) -> design -> implement -> verify on real devices or data -> fix causes and retest on failure -> update README, CLAUDE, PORTING and POLICY in Japanese and English -> push. Write "done" only for what was measured, and say "unverified" honestly for the rest. Exceptions that still need confirmation: irreversible deletions, publishing secrets, anything needing the owner's authentication, payment or real-device actions, and changes to the user's system settings.