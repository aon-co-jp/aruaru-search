# aruaru-search

**Rust 製の自前メタ検索。APIキー不要・VPS で完全無料。** / *Self-hosted, key-free meta-search engine in Rust.*

Google 検索 API の無料枠(1日100件)に頼らず、複数の検索元(Bing・Brave Search・DuckDuckGo・Yahoo! JAPAN など)の
**公開の検索結果ページ**を取得・解析して 1 つに統合し、JSON API として返します。
[aruaru-llm](https://github.com/aon-co-jp/aruaru-llm) とセットで使う前提で、aruaru-llm の検索の第一候補になります。

## 特徴

- **APIキー不要・無料**: 自分の VPS で動かす。外部サービスの契約・課金は不要。
- **世界約130言語に対応**: 英語・アメリカ中心の結果にならないよう、言語ごとに (1) 検索元へ渡す言語・地域を最適化、
  (2) その言語の文字(かな・漢字・ハングル・アラビア文字・キリル文字・デーヴァナーガリー・タイ文字 など)で
  書かれたページを上位へ並べ替え、(3) その言語を得意とする検索元を重く扱います。
  日本語・中国語(簡体/繁体)・韓国語・アラビア語・ペルシア語などで実機確認済みです。
- **検索元の変更に自動で追従**: 毎朝7時(日本時間)と起動時に全検索元を点検し、結果を読み取れなくなった検索元は
  aruaru-llm(無料の AI)にページ構造を見せて CSS セレクタを直してもらいます。**AI の提案はそのまま使わず**、
  設定として正しいか・実際のページで 3 件以上読み取れるかを検証してから取り込み、経緯を `maintenance.log` に残します。
  取得先の URL は AI に変更させません。
- **相手に優しい**: 検索元ごとに間隔を空け、拒否(429 など)されたら 20 分休み、同じ検索は 1 時間キャッシュします。
- **RPoem 上で動く**: HTTP は [RPoem](https://github.com/aon-co-jp/RPoem)(`open-runo-poem-compat`)。

## 使い方

```bash
bash scripts/fetch-deps.sh          # deps.lock どおりに RPoem 等を .deps/ へ取得
cargo run --release                 # 127.0.0.1:4610 で待受
curl -X POST -H "content-type: application/json" -d '{"q":"山梨県 温泉","hl":"ja","n":10}' http://127.0.0.1:4610/aruaru_search.v1.Search/Search
```

API は **Connect の単純呼び出し**(`POST /aruaru_search.v1.<Service>/<Method>`、`content-type: application/json`、本文は JSON、エラーは `{"code","message"}`)。2026-11-04に REST から移行した。

| メソッド | 内容 |
|---|---|
| `Search/Search` `{q, hl, gl, n}` | 検索。`hl` は言語(ja, zh-TW, ar, fa …)、`gl` は地域(省略可)。`{"results":[{title,link,snippet,engines,score}],"warnings":[]}` |
| `Search/Engines` `{}` | 検索元の設定と最近の状態 |
| `Search/Health` `{}` | 使える検索元の状態 |
| `Admin/SelfCheck` `{}` | 点検と自動保守を今すぐ実行(`x-admin-token`。`ARUARU_SEARCH_ADMIN_TOKEN` 設定時のみ) |
| `Search/MediaSearch` `{q, n}` | [archive.org](https://archive.org)を曲名/演奏者/作曲者/レーベル名などで横断検索。**パブリックドメイン・CC0・CC-BY・CC-BY-SA の音源だけ**を返し、即時ストリーミング可能な URL(`stream_url`)も付ける。`{"results":[{identifier,title,creator,date,licenseurl,item_url,stream_url}]}` |
| `GET /healthz` | 死活確認(例外として REST のまま) |

旧 REST の `/v1/search`・`/v1/media-search` は、`aruaru-llm`・`open-english`・`open-upstream-watch` を Connect へ移した(2026-11-04)ので削除した。

環境変数: `ARUARU_SEARCH_ARCHIVE_REPO`(設定と保守履歴の保存先の GitHub 非公開リポジトリ。任意)、`ARUARU_SEARCH_BIND`(既定 `127.0.0.1:4610`)、`ARUARU_SEARCH_DATA_DIR`(既定 `data`)、
`ARUARU_LLM_URL`(既定 `http://127.0.0.1:4600`)、`ARUARU_SEARCH_ADMIN_TOKEN`。

## 回数制限と見張り

- **利用者ごとの回数制限**: プロキシ(open-web-server など)が付ける `X-Forwarded-For` / `X-Real-IP` の IP ごとに、1分20回・1日1,000回
  (`ARUARU_SEARCH_RATE_PER_MIN` / `ARUARU_SEARCH_RATE_PER_DAY`)。超えると HTTP 429(`retry_after` 秒)。
  ヘッダが無い呼び出し(VPS 内の aruaru-llm・realdata.pro)は内部の利用として制限しない(`ARUARU_SEARCH_LOCAL_UNLIMITED=0` で制限)。
  公開するときは、プロキシがクライアントの IP を必ず付け直すこと。
- **検索元のブロックの見張り**: 拒否(429・403・503・確認ページ)されたら休み、続くほど長く休む(20分 → 1時間 → 3時間 → 12時間 → 24時間)。
  成功すれば最初に戻る。状態は `Search/Health` と `Search/Engines` で見られ、拒否はログに出る。

## 曲名/演奏者/作曲者の横断検索(archive.org、Connect `Search/MediaSearch`)

既存の検索元(Bing・Brave など)は「公開の検索結果ページを HTML として解析する」設計だが、
archive.org は構造化された JSON の検索 API(`advancedsearch.php`)とメタデータ API を持つため、
`src/archive_org.rs` に専用モジュールとして切り出している(`archive.rs` が GitHub 保存を担当するのと
同じく、機能ごとにモジュールを分ける既存方針に沿う)。既存の検索元の統合(RRF・意味の並べ替え)には
混ぜず、独立した API として提供する(結果の形が大きく異なる=演奏者・作曲者・ライセンス・
ストリーミング URL を持つため)。

- **ライセンスの絞り込みはこちら側で厳密に行う**: archive.org 自体は非商用限定(CC-BY-NC 等)の
  音源も検索結果に含むため、`licenseurl` を見て、パブリックドメイン(publicdomain/zero・
  publicdomain/mark)・CC0・CC-BY・CC-BY-SA だけを通す(CC-BY-NC・CC-BY-ND・CC-BY-NC-SA・
  CC-BY-NC-ND・ライセンス不明は除外)。`licenseurl` が無い(=ライセンス表示の無い)音源も除外する。
- 上位の結果だけ、メタデータ API(`metadata/<identifier>`)からファイル一覧を取得し、
  再生可能な音声ファイル(mp3 → Ogg Vorbis → FLAC の順で優先)の直接ダウンロード URL を
  `stream_url` として埋める(全件で行うと重いため上位のみ)。
- 実機確認例: `curl -XPOST -H 'content-type: application/json' -d '{"q":"Caruso","n":5}' http://127.0.0.1:4610/aruaru_search.v1.Search/MediaSearch`(Connect、POST、RS-JSON) で、
  Enrico Caruso のパブリックドメイン録音(1908〜1919年、`publicdomain/mark`)が
  ストリーミング URL 付きで返ることを確認済み(2026-09-27)。

## 休止からの自動再開

検索元が拒否・混雑・ネットワーク障害で休止しても、**時刻が来ると固定の無害な語で探りの検索を投げ、成功すれば休止の途中でも即復帰**します
(失敗なら指数バックオフ、休止の理由=拒否/429/ネットワークで長さを変えます)。`POST /aruaru_search.v1.Search/Health` の各検索元に
`cooldown_reason`・`next_probe_secs`・`last_probe` が出ます。詳細は `CLAUDE.md`。

## 注意

- 各検索元の利用規約と `robots.txt` の範囲で、個人・研究・小規模な用途を想定しています。大量の自動取得には使わないでください。
- 検索元は機械による利用を制限することがあります(DuckDuckGo は短時間に多数の検索をすると確認ページを返します)。
  そのため複数の検索元に分散し、拒否された検索元は休ませます。
- ライセンス: MIT OR Apache-2.0

## English

aruaru-search fetches and parses the public result pages of several search engines (Bing, Brave Search,
DuckDuckGo, Yahoo! JAPAN …), merges them with reciprocal-rank fusion, and serves a JSON API — no API keys, no fees.
Around 130 languages are supported: hl/gl are normalised per engine, results written in the query language's script
are ranked above unrelated-language pages, and engines strong in that language get more weight. Every morning
(07:00 JST) and at start-up all engines are self-checked; when an engine's page layout changes, the CSS selectors
are repaired with the free AI via aruaru-llm — AI proposals are validated against the real page before being applied,
and the fetch URL is never AI-editable.

## JSON処理の使用方針: `parse_secure_sonic`(速い型+安全)と`sonic-rs`は使える所では全て使う(2026-10-14、ユーザー指示)
正本は[`RS-JSON`](https://github.com/aon-co-jp/RS-JSON)(`README.md`・`PORTING.md`)。

**方針**: JSONを読む所は、使える所では**全て** `rust-json`(RS-JSON)の `parse_secure_sonic`(`fast`機能、結果は`sonic_rs::Value`)を使う。
`sonic-rs` を直接使える所(型付き構造体への読み込みなど)も同様。使えない所は `parse_secure`(結果は`serde_json::Value`)を使う。
外部から来る信頼できない入力(HTTP本文・ネットワーク越しのファイル・アップロード)は、**必ずサイズ・深さ・重複キーの上限つき**で読む。

**実測(7 MB・4万件)**: `serde_json` 200 ms / `parse_secure` 217 ms / **`parse_secure_sonic` 83 ms(約2.4倍速い、防御込み)** / `sonic-rs`そのまま 33 ms(約6倍、防御なし)。
`serde_json::Value`に読む限りSIMDでもほぼ速くならない(188 ms)。**速さの源は、結果を連続した領域に詰め込む値の型**。

**使える所 / 使えない所**
- ✅ 読み取り中心で、JSONを大きく・頻繁に読む所(キャッシュJSON、設定、APIの応答、ログ、インデックス)。
- ✅ 外部からの入力の入口(速さより防御が目的。`parse_secure`でも可)。
- ⚠️ `serde_json::Value`を関数間で受け渡している既存コードは、型を`sonic_rs::Value`に直す書き換えが要る(`JsonValueTrait`/`JsonContainerTrait`のメソッド)。書き換えずに`serde_json::Value`へ読むと速くならない。
- ❌ WASM(`rust_json::light`、依存ゼロ)、PHP/JavaScript側のJSON処理(Rustの部品は使えない)。
- 注意: 依存先の`sonic-rs`は内部で`unsafe`を使う(自前コードに`unsafe`は無い)。小さなJSON(数KB)では体感差が出ない。速度目的の書き換えは、実測で効く所から。

**導入**: `Cargo.toml`に `rust-json = { path = "../RS-JSON", features = ["fast"] }`。使用例は`RS-JSON/README.md`。

**このリポジトリでの適用状況**: 未適用(方針のみ)。コードの切り替えは、JSONを大きく・頻繁に読む所から実測して順に行う。

## JSON handling policy: use `parse_secure_sonic` (fast type + secure) and `sonic-rs` everywhere they can be used (2026-10-14, user instruction)
The source of truth is [`RS-JSON`](https://github.com/aon-co-jp/RS-JSON) (`README.md`, `PORTING.md`).

**Policy**: wherever JSON is read, use `parse_secure_sonic` from `rust-json` (RS-JSON, the `fast` feature; the result is `sonic_rs::Value`) **everywhere it can be used**.
Use `sonic-rs` directly where possible too (e.g. reading into typed structs). Where it cannot be used, use `parse_secure` (result: `serde_json::Value`).
Untrusted input from outside (HTTP bodies, files over the network, uploads) **must always be read with size / depth / duplicate-key limits**.

**Measured (7 MB, 40,000 records)**: `serde_json` 200 ms / `parse_secure` 217 ms / **`parse_secure_sonic` 83 ms (about 2.4x faster, with the defences)** / raw `sonic-rs` 33 ms (about 6x, no defences).
Reading into `serde_json::Value` gives almost no speed-up even with SIMD (188 ms). **The speed comes from the value type that packs the result into contiguous memory.**

**Where it can / cannot be used**
- ✅ Read-heavy code that parses large or frequent JSON (cache JSON, config, API responses, logs, indexes).
- ✅ Entrances for outside input (the goal there is defence rather than speed; `parse_secure` is fine).
- ⚠️ Existing code that passes `serde_json::Value` between functions needs its type changed to `sonic_rs::Value` (methods from `JsonValueTrait` / `JsonContainerTrait`). Reading into `serde_json::Value` without that change is not faster.
- ❌ WASM (`rust_json::light`, zero dependencies) and the JSON handling on the PHP / JavaScript side (Rust components cannot be used there).
- Note: the `sonic-rs` dependency uses `unsafe` internally (our own code has none). For small JSON (a few KB) the difference is not noticeable; do speed-motivated rewrites where measurements show a benefit.

**Adoption**: in `Cargo.toml`, `rust-json = { path = "../RS-JSON", features = ["fast"] }`. Example in `RS-JSON/README.md`.

**Status in this repository**: Not yet applied (policy only). Code will be switched over place by place, starting where measurements show large or frequent JSON reads.

---

作者 / Author: 石 塚 正 浩（MASAHIRO ISHIZUKA） (https://github.com/aon-co-jp)

## API 方針: REST 全廃・gRPC / Connect へ (2026-11-04)

### 日本語
- **REST は全廃する。新しい API は REST で作らない。** ブラウザや `curl` から呼ぶ API は **Connect の単純呼び出し**(`POST /<パッケージ>.<Service>/<Method>`、`content-type: application/json`、本文は JSON)、サービス間は **gRPC / gRPC-web**(RPoem の管理API `open_runo.admin.v1.Admin` と同じ流儀)。ストリーミングは、必要になってから。
- **既存の REST は、触るときに順に移す。** 移したら旧 REST の経路は消す。移行が終わるまで残るが、**新規には追加しない**。
- **JSON は RS-JSON に統一**する(読み込みは `from_str_secure` / `parse_secure` でサイズ・深さ・重複キーの上限つき、書き出しは `rust_json::fast`)。ブラウザの WASM に `prost` は入れない(サイズが増えるため)。
- **例外(自分たちの API ではなく、相手の規格が決めているもの)**: ACME、OAuth/OIDC のコールバック、第三者(GitHub など)からの Webhook、ヘルスチェック、静的ファイル配信、OTLP など。これらは規格どおりの HTTP を受ける。
- 参考実装: `RPoem`(`open_runo.admin.v1.Admin`)、`open-easy-web`(`server/src/connect.rs`、`proto/open_easy_web/admin/v1/admin.proto`)。
- ※ このリポジトリのコードが既に方針どおりかどうかは、各リポジトリの現状による。この節は方針の記録であり、コードの変更を意味しない。

### English
- **REST is being abolished. New APIs are not built as REST.** APIs called from browsers or `curl` use **Connect unary calls** (`POST /<package>.<Service>/<Method>`, `content-type: application/json`, JSON body); service-to-service calls use **gRPC / gRPC-web** (the same style as RPoem's admin API `open_runo.admin.v1.Admin`). Streaming only when it is actually needed.
- **Existing REST endpoints are migrated one by one, when they are touched.** The old REST route is removed once migrated. Until migration is finished they remain, but **no new ones are added**.
- **JSON is unified on RS-JSON**: reading with `from_str_secure` / `parse_secure` (size, depth and duplicate-key limits), writing with `rust_json::fast`. `prost` is not put into browser WASM (it would grow the binary).
- **Exceptions (shapes dictated by someone else's standard, not our own API)**: ACME, OAuth/OIDC callbacks, webhooks from third parties (e.g. GitHub), health checks, static file serving, OTLP, and so on. These accept plain HTTP as the standard requires.
- Reference implementations: `RPoem` (`open_runo.admin.v1.Admin`) and `open-easy-web` (`server/src/connect.rs`, `proto/open_easy_web/admin/v1/admin.proto`).
- Note: whether the code in a given repository already follows this policy depends on that repository. This section records the policy; it does not mean the code has been changed.

### Deutsch (KI-Übersetzung, redaktionell überarbeitet; noch nicht von Muttersprachlern geprüft)
- **REST wird vollständig abgeschafft; neue APIs entstehen nicht mehr als REST.** APIs, die aus dem Browser oder per `curl` aufgerufen werden, verwenden **unäre Connect-Aufrufe** (`POST /<Paket>.<Service>/<Methode>`, `content-type: application/json`, JSON-Body). Zwischen Diensten kommen **gRPC bzw. gRPC-web** zum Einsatz, im selben Stil wie die Admin-API von RPoem (`open_runo.admin.v1.Admin`). Streaming wird erst ergänzt, wenn es tatsächlich gebraucht wird.
- **Bestehende REST-Endpunkte werden schrittweise umgestellt, und zwar immer dann, wenn man ohnehin an ihnen arbeitet.** Sobald ein Endpunkt umgestellt ist, wird die alte REST-Route entfernt. Bis zum Abschluss der Migration bleiben die übrigen bestehen, es werden aber **keine neuen mehr hinzugefügt**.
- **JSON wird einheitlich über RS-JSON verarbeitet**: Gelesen wird mit `from_str_secure` bzw. `parse_secure` (mit Obergrenzen für Größe, Verschachtelungstiefe und doppelte Schlüssel), geschrieben mit `rust_json::fast`. `prost` wird nicht in das Browser-WASM aufgenommen, da es die Datei vergrößern würde.
- **Ausnahmen (Schnittstellen, deren Form ein fremder Standard vorgibt und die nicht zu unserer eigenen API gehören)**: ACME, OAuth/OIDC-Callbacks, Webhooks von Drittanbietern (z. B. GitHub), Health-Checks, die Auslieferung statischer Dateien, OTLP und Ähnliches. Diese nehmen, wie vom jeweiligen Standard verlangt, gewöhnliches HTTP entgegen.
- Referenzimplementierungen: `RPoem` (`open_runo.admin.v1.Admin`) und `open-easy-web` (`server/src/connect.rs`, `proto/open_easy_web/admin/v1/admin.proto`).
- Hinweis: Ob der Code eines einzelnen Repositorys diese Richtlinie bereits erfüllt, hängt vom jeweiligen Repository ab. Dieser Abschnitt dokumentiert lediglich die Richtlinie; er besagt nicht, dass der Code schon angepasst wurde.

### Русский (перевод выполнен с помощью ИИ и отредактирован; носителями языка пока не проверен)
- **От REST мы полностью отказываемся; новые API больше не создаются в стиле REST.** API, к которым обращаются из браузера или через `curl`, используют **унарные вызовы Connect** (`POST /<пакет>.<Service>/<Method>`, `content-type: application/json`, тело запроса — JSON). Для взаимодействия между сервисами применяются **gRPC и gRPC-web** — в том же стиле, что и административный API RPoem (`open_runo.admin.v1.Admin`). Потоковая передача данных добавляется лишь тогда, когда в ней действительно возникает необходимость.
- **Существующие REST-эндпоинты переводятся поэтапно — по мере того, как с ними приходится работать.** После перевода старый REST-маршрут удаляется. Пока миграция не завершена, оставшиеся эндпоинты сохраняются, но **новые не добавляются**.
- **Работа с JSON унифицирована на основе RS-JSON**: чтение — через `from_str_secure` или `parse_secure` (с ограничениями на размер, глубину вложенности и повторяющиеся ключи), запись — через `rust_json::fast`. `prost` в браузерный WASM не включается, поскольку это увеличило бы размер файла.
- **Исключения (интерфейсы, форму которых определяет чужой стандарт, а не наш собственный API)**: ACME, обратные вызовы OAuth/OIDC, вебхуки сторонних сервисов (например, GitHub), проверки работоспособности, раздача статических файлов, OTLP и тому подобное. Такие интерфейсы принимают обычный HTTP, как того требует соответствующий стандарт.
- Эталонные реализации: `RPoem` (`open_runo.admin.v1.Admin`) и `open-easy-web` (`server/src/connect.rs`, `proto/open_easy_web/admin/v1/admin.proto`).
- Примечание: соответствует ли код конкретного репозитория этой политике, зависит от самого репозитория. Данный раздел лишь фиксирует политику и не означает, что код уже изменён.

## API 方針の補足: 用語の整理と使い分け (2026-11-04)

### 日本語
**1つに統一するものではなく、別の層の話です。** 「REST の代わり」になるのは Connect・gRPC・GraphQL だけで、WASM は画面を作る技術です。

| 用語 | 何か | REST の代わりか |
|---|---|---|
| REST | URL と HTTP メソッドで操作を表す API の作り方 | 全廃する側 |
| **Connect** | `POST /サービス名/メソッド名` に JSON を送る形式。`curl` でもブラウザの `fetch` でも呼べる | はい(**採用**) |
| **gRPC** | 同じ「サービス/メソッド」の考え方を、バイナリ(protobuf)と HTTP/2 で行う形式。速く、ストリーミングも得意 | はい(**サービス同士に採用**) |
| gRPC-web | gRPC のブラウザ向けの変形版 | はい(必要なら) |
| GraphQL | 欲しい項目を問い合わせる形式 | 一部だけ(**問い合わせ(読み取り)専用**) |
| WASM | Rust をブラウザの中で動かす仕組み | いいえ(画面を作る技術) |

- **ブラウザの画面や `curl` から呼ぶ API は Connect**(POST、JSON を RS-JSON で読み書き)。ブラウザの WASM は素の gRPC を話せず、Connect なら `fetch` のままで、`prost` を足さずに済む(画面のサイズも増えない)。
- **サービス同士の通信(サーバー間、重い処理、ストリーミング)は gRPC。** 多くは Connect で足りるので、ストリーミングが必要になった所だけ gRPC に上げる。
- **GraphQL は、まとめて問い合わせたいときの読み取り専用。** 書き込み(操作)には使わない。
- **WASM は画面を作る技術として、そのまま使う。** 画面から呼ぶ API が Connect になっただけ。
- 外部の規格が形を決めるもの(ACME、OAuth のコールバック、第三者の Webhook、ヘルスチェック、静的ファイル、OTLP)は、規格どおりの HTTP のまま。
- **迷ったら Connect。**

### English
**These are not alternatives to unify; they sit at different layers.** Only Connect, gRPC and GraphQL can replace REST; WASM is a way to build the UI.

| Term | What it is | Replaces REST? |
|---|---|---|
| REST | API style that expresses operations through URLs and HTTP methods | The thing being abolished |
| **Connect** | `POST /Service/Method` with a JSON body; callable from `curl` and from a browser `fetch` | Yes (**adopted**) |
| **gRPC** | The same service/method idea over binary (protobuf) and HTTP/2; fast, good at streaming | Yes (**for service-to-service**) |
| gRPC-web | A browser-friendly variant of gRPC | Yes (if needed) |
| GraphQL | A query language: ask for exactly the fields you want | Partly (**read-only queries**) |
| WASM | Runs Rust inside the browser | No (a UI technology) |

- **APIs called from browser screens or `curl` use Connect** (POST, JSON read and written with RS-JSON). Browser WASM cannot speak plain gRPC; with Connect it stays a `fetch`, and `prost` is not needed (the binary does not grow).
- **Service-to-service calls (server to server, heavy work, streaming) use gRPC.** Connect is enough in most places; move a call to gRPC only where streaming is actually needed.
- **GraphQL is read-only**, for queries that gather several pieces of data. It is not used for writes (operations).
- **WASM stays as the way to build the UI.** The API it calls simply became Connect.
- Interfaces whose shape an outside standard dictates (ACME, OAuth callbacks, third-party webhooks, health checks, static files, OTLP) stay plain HTTP.
- **When in doubt, use Connect.**

### Deutsch (KI-Übersetzung, sprachlich überarbeitet; noch nicht von Muttersprachlern geprüft)
**Es geht nicht darum, eine einzige Technik auszuwählen – die Begriffe liegen auf unterschiedlichen Ebenen.** Als Ersatz für REST kommen nur Connect, gRPC und GraphQL in Frage; WASM dagegen ist eine Technik, mit der die Benutzeroberfläche gebaut wird.

| Begriff | Was ist das? | Ersatz für REST? |
|---|---|---|
| REST | Ein API-Stil, bei dem Operationen über URLs und HTTP-Methoden ausgedrückt werden | wird abgeschafft |
| **Connect** | `POST /Dienst/Methode` mit JSON im Body; aufrufbar mit `curl` ebenso wie mit `fetch` im Browser | ja (**gewählt**) |
| **gRPC** | Dasselbe Prinzip aus Dienst und Methode, aber über binäres protobuf und HTTP/2; schnell und gut für Streaming geeignet | ja (**zwischen Diensten**) |
| gRPC-web | Eine für den Browser angepasste Variante von gRPC | ja (bei Bedarf) |
| GraphQL | Abfragesprache, mit der man genau die benötigten Felder anfordert | teilweise (**nur für lesende Abfragen**) |
| WASM | Führt Rust-Code direkt im Browser aus | nein (Technik für die Oberfläche) |

- **APIs, die von Browseroberflächen oder per `curl` aufgerufen werden, verwenden Connect** (POST; JSON wird mit RS-JSON gelesen und geschrieben). WASM im Browser kann kein reines gRPC sprechen. Mit Connect bleibt es bei einem gewöhnlichen `fetch`, und `prost` ist nicht nötig – die WASM-Datei wird dadurch nicht größer.
- **Für die Kommunikation zwischen Diensten (Server zu Server, rechenintensive Aufgaben, Streaming) wird gRPC eingesetzt.** In den meisten Fällen reicht Connect aus; auf gRPC wird nur dort umgestellt, wo Streaming tatsächlich gebraucht wird.
- **GraphQL dient ausschließlich lesenden Abfragen**, wenn mehrere Informationen gebündelt abgefragt werden sollen. Für Schreibzugriffe (Operationen) wird es nicht verwendet.
- **WASM bleibt die Technik für die Oberfläche.** Geändert hat sich lediglich, dass die aufgerufene API nun auf Connect beruht.
- Schnittstellen, deren Form durch einen externen Standard festgelegt ist (ACME, OAuth-Callbacks, Webhooks von Drittanbietern, Health-Checks, statische Dateien, OTLP), bleiben gewöhnliches HTTP.
- **Im Zweifel: Connect.**

### Русский (перевод выполнен с помощью ИИ и литературно отредактирован; носителями языка пока не проверен)
**Это не варианты, из которых нужно выбрать один, а технологии разных уровней.** Заменить REST могут только Connect, gRPC и GraphQL; WASM же — это способ создания пользовательского интерфейса.

| Термин | Что это | Заменяет REST? |
|---|---|---|
| REST | Стиль построения API, при котором операции выражаются через URL и HTTP-методы | отменяется |
| **Connect** | `POST /Сервис/Метод` с телом в формате JSON; вызывается как из `curl`, так и из браузерного `fetch` | да (**выбран**) |
| **gRPC** | Тот же принцип «сервис — метод», но поверх двоичного формата protobuf и HTTP/2; работает быстро и хорошо подходит для потоковой передачи | да (**для связи между сервисами**) |
| gRPC-web | Адаптированный для браузера вариант gRPC | да (при необходимости) |
| GraphQL | Язык запросов: клиент запрашивает ровно те поля, которые ему нужны | частично (**только запросы на чтение**) |
| WASM | Позволяет выполнять код на Rust прямо в браузере | нет (технология для интерфейса) |

- **API, которые вызываются из браузерных интерфейсов или через `curl`, строятся на Connect** (POST; JSON читается и записывается через RS-JSON). Браузерный WASM не умеет работать с «чистым» gRPC. С Connect достаточно обычного `fetch`, а `prost` не требуется — размер WASM-файла не увеличивается.
- **Для обмена между сервисами (сервер — сервер, ресурсоёмкие задачи, потоковая передача) применяется gRPC.** В большинстве случаев достаточно Connect; на gRPC переходят только там, где потоковая передача действительно необходима.
- **GraphQL используется только для чтения**, когда нужно получить сразу несколько связанных данных. Для записи (операций) он не применяется.
- **WASM по-прежнему остаётся технологией для интерфейса.** Изменилось лишь то, что вызываемый им API теперь построен на Connect.
- Интерфейсы, форма которых задана внешним стандартом (ACME, обратные вызовы OAuth, вебхуки сторонних сервисов, проверки работоспособности, статические файлы, OTLP), остаются обычным HTTP.
- **Если есть сомнения — выбирайте Connect.**
