# aruaru-search 開発方針

作業ドライブは `F:\aruaru-search`。共通ルールは [`open-raid-z/CLAUDE.md`](https://github.com/aon-co-jp/open-raid-z) を正本とする。

## 役割
Rust 製の自前メタ検索(APIキー不要)。aruaru-llm の検索の第一候補として、VPS で aruaru-llm とセットで動かす。

## 設計上の約束
- 検索元の違いは**コードではなく設定**(`engines.default.json` / `data/engines.json`)。ページの作りが変わったら設定(CSS セレクタ)を直す。
- AI が1回の回答で直接変えてよいのはセレクタと転送パラメータ名だけ。**URL・重みは AI に変えさせない**。提案は実ページで3件以上読み取れると確認してから取り込む(`maintain.rs`)。
  無効化だけは例外で、AI が「セレクタでは直せない」(`give_up`)と`DISABLE_AFTER_GIVE_UPS`(既定3)回**連続で**判断したときに限り、
  自動で `enabled: false` にする(2026-09-30、CAPTCHAゲートされたBaiduの実例を受けて追加)。1回の一時的な失敗だけでは無効化しない。
  無効化した検索元は`maintenance.log`に理由を残し、手動で`enabled`をtrueに戻すまで再度使われない。
- 英語・アメリカ中心にしない: 言語ごとの `hl/gl` 正規化・文字種による並べ替え・得意な検索元の重み付け(`lang.rs`)を崩さない。新しい言語の対応は `languages.rs`(約130言語)と `lang.rs` の表に足す。
- 検索元に負担をかけない: 間隔(`MIN_INTERVAL`)・拒否後の休止(`COOLDOWN`)・キャッシュを弱めない。
- 依存は `deps.lock` でコミット固定し `.deps/` に取得(`scripts/fetch-deps.sh`)。`cargo fmt --all` は使わず `cargo fmt-own` を使う。

## 未実施・次の候補
- (実装済み・要有効化)設定・保守履歴の GitHub 非公開リポジトリへの保存(`ARUARU_SEARCH_ARCHIVE_REPO`)。**aruaru-db・VPS のディスクには保存しない**(ユーザー指示 2026-09-25)。
- 結果の意味的な並べ替え(open-cuda 等の計算資源を使う埋め込み)。

## 日本語検索元の追加調査(2026-09-28〜29)

Bing・Brave・Yahoo! JAPAN の3つしかなかった日本語検索元に、実ページでセレクタを
検証した上で **エキサイト ウェブ検索**(`websearch.excite.co.jp`、id=`excite`)を追加した
(`engines.default.json`・`src/engine.rs`)。robots.txt で検索結果ページ自体は禁止されて
いないことを確認済み。VPS で実機確認済み(`Search/Health` で `excite` が healthy、実クエリの
結果にも混ざることを確認)。

**この回で調べて見送った候補**(理由つき。次に探すときに同じ道を通らないための記録):
- **goo検索**(search.goo.ne.jp) — VPS から DNS 解決不可。ドメイン自体が消滅しており、
  サービス自体が終了したとみられる。
- **BIGLOBEサーチ** — アクセスすると「BIGLOBEサーチは終了しました。」の告知ページのみ。
- **Infoseek** — 検索機能自体が無くなり、今は楽天運営のニュースサイトになっている。
- **@nifty検索**(search.nifty.com) — ページは開けるが、結果が JS 描画でしか出ず、
  静的 HTML の取得(現行の `scraper` ベースの実装)では結果が拾えない。
- **Startpage** — 結果は正常に取得できたが、robots.txt が検索結果のパス `/sp/` を
  明示的に `Disallow` しているため、既存の方針(robots.txt で禁止されていないことを
  確認してから使う)に反するため見送り。
- **Yandex** — 結果は取得できたが、robots.txt が `/search` を明示的に `Disallow`
  しているため見送り(前回リストにあった「ロシア(Yandex)」の候補はこれで確認・却下)。
- **Sogou(搜狗)** — アクセス直後にアンチスパイダー(ボット検知)ページへリダイレクトされ、
  そもそも取得不可。中国語中心のため日本語検索元としての優先度も元々低い。
- **Ecosia** — Cloudflare のボット対策(チャレンジページ)でブロックされる。
- **Qwant** — サービス自体が "Service unavailable" で利用不可。

**結論**: 日本語の独立した検索エンジン市場は非常に限られており(実質 Google・Bing 系列に
集約)、今回の調査で見つかった追加候補は Excite のみだった。今後さらに候補を探す場合は、
上記で見送った理由(サービス終了・JS描画・robots.txt禁止・ボット検知)に当てはまらない
新しいものを探す必要がある。日本語検索元が Bing・Brave・Yahoo! JAPAN・Excite の4つに
増えたことで、休止が重なったときの心許なさは多少改善したが、根本的な解消ではない。

## AI によるセレクタ自動修復・無効化(2026-09-30)

`realdata.pro` の実運用で、9/29 の毎朝の自動収集が23か所中2か所で打ち切られる事象が見つかり
(詳細は `realdata.pro/CLAUDE.md` の再開用メッセージ)、原因の一つとして **Baidu が CAPTCHA
ゲート**(`wappass.baidu.com` への強制リダイレクト、VPS の IP に対して継続的)されていることが
判明した。これはセレクタの修正では直せない(そもそもページに結果が無い)ため、`enabled: false`
に変更(`rev`を上げて保存済み設定にも反映されるようにした)。

これを機に、`maintain.rs` の自己修復に**AI 自身の無効化判断**を追加した:
- AI が `{"give_up":true}`(セレクタでは直せない、CAPTCHA・JS必須・利用拒否などの可能性)と
  `DISABLE_AFTER_GIVE_UPS`(既定3)回**連続で**判断した検索元は、自動的に `enabled: false` にする。
- 取得したページの中身がほぼ空(AI に見せられないほど小さい、まさに CAPTCHA リダイレクトのような
  ケース)も、AI に問い合わせるまでもなく give_up 扱いにする(`repair` 関数)。
- 1回でも直った・正常に読み取れた時点で `give_up_streak` は 0 に戻る(一時的な失敗の連鎖では
  無効化されない)。
- 無効化した経緯は `maintenance.log` に残る。
- **自動復活(2026-09-30追加)**: 無効化中の検索元も、毎回の点検(`selfcheck`)で見本1件だけ
  軽く試す(`recheck_disabled`)。負担をかけないよう全見本ではなく1件だけ。読み取れれば
  `enabled:true`・`give_up_streak:0` に戻し、`maintenance.log` に `re_enabled` として記録する。
  CAPTCHAゲートやサービス側の一時的な制限が後で解除された場合、手動操作なしに自然と復活する。
- `EngineDef.give_up_streak`(`engine.rs`)・`RepairOutcome`/`GiveUp`/`bump_give_up_streak`/
  `reset_give_up_streak`/`recheck_disabled`(`maintain.rs`)で実装。テスト
  (`give_up_streak_disables_after_threshold_and_resets_on_recovery`など)で、しきい値未満では
  無効化しないこと・しきい値到達で無効化されること・回復で streak が0に戻ることを確認済み。

## archive.org 横断検索(Connect `Search/MediaSearch`、2026-09-27 実装・実機確認済み)

曲名・演奏者・作曲者・レーベル名で archive.org を検索し、パブリックドメイン・CC0・CC-BY・
CC-BY-SA の音源だけを即時ストリーミング可能な形(`stream_url`)で返す新エンドポイント。
既存の `engine.rs`(HTML 解析前提)には混ぜず、`src/archive_org.rs` に専用モジュールとして
実装した(archive.org は構造化 JSON API を持つため)。ライセンスの絞り込みは
`is_allowed_license()` でこちら側が厳密に行う(archive.org 自体は CC-BY-NC 等の非商用限定
音源も検索結果に含むため)。実機で `curl -XPOST -H 'content-type: application/json' -d '{"q":"Caruso","n":5}' http://127.0.0.1:4610/aruaru_search.Search/MediaSearch`
を確認し、Enrico Caruso のパブリックドメイン録音がストリーミング URL 付きで返ることを確認済み。

- VPS へ反映済み(2026-11-05、Connect 版 `38d6e3a`)。aruaru-llm の `Llm/MediaSearch` 経由でも実機確認済み。`open-bar`/`open-music-llm`
  等、実際に音源を再生・利用する側からこの API を呼ぶ導線はまだ無い。
- **未実施**: 大量アクセス時の archive.org 側への配慮(現状は個別の HTTP リクエストのみで、
  既存の検索元のような間隔調整・拒否時の休止は archive.org 向けには実装していない。
  archive.org は比較的緩やかな API だが、将来的に負荷が増えるなら追加を検討)。

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

## 英語の検索元の追加(2026-10-16)

**背景**: VPS の IP から Bing・Brave が拒否されて休止すると、英語の検索は残りの検索元(日本語・韓国語・中国語向け)が
全て対象外のため、結果が0件(502)になっていた(aruaru-llm の共有 AI 経路で実測)。ユーザー指示で英語向けの検索元を追加した。

**HTML の検索結果ページを読む英語の検索元は、方針(`robots.txt` で禁止されていない・ボット検知されない)に合うものが無かった**
(2026-10-16 に VPS から実測。次に同じ道を通らないための記録):
- **Mojeek** — `robots.txt` が `/search` を禁止、実ページも HTTP 403(正規の用途は有料 API)。
- **Yahoo! 全世界版**(search.yahoo.com) — `robots.txt` が `/search` を禁止。
- **Marginalia** — `robots.txt` が `/search` を禁止(移転先 marginalia-search.com も同じ)。
- **Yep** — HTTP 403。**DuckDuckGo Lite**・**Dogpile** — HTTP 202(人かどうかの確認)で結果が出ない。
- **Ask.com**・**AOL 検索** — サービス終了(GitHub Pages の404ページ・404)。

**そこで、プログラムからの利用が公式に認められた無料の公開 API(JSON)を検索元にした**(`EngineDef.kind`):
- `wikipedia` — MediaWiki API。**検索語の言語のサブドメイン**(`{lang}.wikipedia.org`、`{lang}` は `hl` の基本コード)を使うので、
  約130言語の検索にそのまま使える。
- `hn` — Hacker News の Algolia API(英語のみ)。
- `stackoverflow` — Stack Exchange API の `/search/excerpts`(英語のみ。キー無しは1日300回までのため間隔3秒、
  超過時の HTTP 400 は長めに休む)。
- API の検索元は **セレクタ・AI による自動修復・「検索語の一部しか反映されない」判定の対象外**(`canaries_for` が空)。
  `User-Agent` はブラウザを装わず `aruaru-search/1.0 (+リポジトリURL)` と名乗る(Wikimedia のポリシー)。
- **限界(正直な開示)**: これらは百科事典・プログラミングの Q&A・技術ニュースに強いが、一般のウェブ全体の最新情報
  (例: 製品の最新バージョン)は Bing・Brave に頼る。両方が休止中なら、一般の話題は0件になりうる。

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

## Connect 移行の反映状況(2026-11-05)
VPS の aruaru-search は Connect 版(`38d6e3a`)。旧RESTは削除済み。呼び出し元(aruaru-llm・open-upstream-watch・realdata.pro・`diet_crawl.py`)も Connect(POST、RS-JSON)。`/healthz` は例外として残す。外部から来る JSON は `from_str_secure` で読む。

## 休止からの自動再開(2026-11-06)

**背景**: bing は91分・brave は111分など(`blocked_level` 2)休止したまま戻らず、aruaru-jobs の求人検索が「free search unavailable」になった。
休止は「拒否されたら一定時間(20分→1時間→3時間→12時間→24時間)使わない」だけで、**相手が復帰したかを確かめる手段が無く、満了を待つだけ**だった。

**従来の休止ロジック(調査結果、`src/search.rs`)**
| 契機 | 休止 | 段階の上限 |
|---|---|---|
| HTTP 202 / 403 / 429 / 503、API の 400 | `COOLDOWN_STEPS`(20分〜24時間) | 4 |
| 人かどうかの確認画面 | 同上 | 4 |
| 検索語の一部しか反映されない結果(ソフトブロック) | 同上 | 2(最長3時間) |
| 接続失敗・タイムアウト | **休止しない**(毎回失敗を待つ) | - |
成功すると段階は 0 に戻る。休止中の `fetch_page` は即座に失敗を返す。時刻は `Instant`(試験で差し替え不可)。

**新しい仕組み**
- **休止の理由を区別**(`BlockKind`): `rejected`(拒否、従来の長い段階)/ `rate_limited`(429、5分〜3時間)/ `network`(接続失敗・タイムアウト・500/502/504、30秒〜10分)。
- **探りの検索**(`probe_engine`): 休止中の検索元へ、固定の無害な語 `wikipedia`(`PROBE_QUERY`。秘密も利用者の検索語も含まない)を投げ、結果を読み取れれば**即復帰**(休止を解き、段階を1つ下げて残す)。
  失敗なら指数バックオフ(基本間隔×2^失敗回数。拒否 10分→2時間、429 5分→30分、ネットワーク 20秒→2分が上限)。**探りの失敗で休止の期限は延ばさない**(拒否を深めない)。次の探りは休止の満了時刻を超えない。
- **定期実行**: `main.rs` が10秒ごとに `probe_due(None)` を呼ぶ(時刻が来た休止中の検索元だけ探る)。
- **素早い切り替え**: 検索で言語向けの検索元がすべて休止中なら、探りの時刻が来たものを今すぐ探り(最大25秒)、それでも駄目なら言語を限らない他の有効な検索元へ広げる。
- **時計の差し替え**: `Searcher::with_clock`。休止の時刻はすべて UNIX 秒で、試験は偽の時計と手元の模擬サーバーで検証する。
- **応答**: `Search/Health` の各検索元に `cooldown_reason`・`next_probe_unix`・`next_probe_secs`・`probe_failures`・`last_probe`(`unix`/`ok`/`error`)を追加。`Search/Engines` の `status` にも同名の項目が出る。既存の項目は不変。
- **約束**: 確認画面の突破はしない。探りは休止中の検索元にだけ、少ない回数で行う。

## 2026-11-20 低VRAM・VRAM無し端末(VPS・PC・タブレット・スマホ)でのAI/LLMのノウハウ / Low-VRAM AI know-how
日本語: AI・LLMを使う機能(推論・要約・補完・OCR等)は、[aruaru-llm/docs/LOW-VRAM-KNOWHOW.md](https://github.com/aon-co-jp/aruaru-llm/blob/main/docs/LOW-VRAM-KNOWHOW.md) に従う: 短い文脈・上限・タイムアウト、VPSで重い推論を直接回さない、メモリが足りなければ小さいモデルへ自動で下げる。広い桁(FP64以上)は本番で使わず、試験の正解にだけ使う。
English: Features that use AI/LLMs follow the file above: short context, caps and timeouts, no heavy inference directly on the VPS, auto-downgrade to a smaller model when memory is short. Wide floats (FP64 and above) are for test oracles only, not production.

## 完成状況 / Status (2026-11-22)
**日本語**: Connect 移行は完了し VPS に反映済み。`ok()` は RS-JSON の書き出し結果をそのまま返す(二重変換を廃止)。Baidu は既定の `rev` を 2 に上げて無効を反映した。方針の要約は [POLICY.md](POLICY.md)。未確認: `maid-cafe-programming-school` の `extension.ts` が aruaru-llm 経由であること(コード未確認)。
**English**: The Connect migration is complete and deployed. `ok()` returns RS-JSON's bytes as they are (double conversion removed). Baidu's default `rev` was raised to 2 so its disabled state propagates. See [POLICY.md](POLICY.md). Unverified: that `maid-cafe-programming-school`'s `extension.ts` goes through aruaru-llm (not checked in code).

## 完成(2026-11-24) / Done
**日本語**: REST 全廃・Connect 移行は完了(VPS 反映済み、`/v1/search`・`/v1/health` は404、`/healthz` は200)。試験 50 件成功、拡張機能は `npm run compile` 成功。未実施として残すもの: 拡張機能の再パッケージ・公開(利用者側の更新が済むまで旧版は aruaru-llm の旧 REST で 404)、VS Code 上での実機操作。
**English**: REST abolition and the Connect migration are complete (deployed; `/v1/search` and `/v1/health` return 404, `/healthz` 200). 50 tests pass; the extension compiles with `npm run compile`. Left undone: repackaging/publishing the extension (old versions get 404 from aruaru-llm's old REST until users update) and hands-on checks inside VS Code.

## 2026-11-24 MediaSearch の探し直し / MediaSearch relaxation
**日本語**: `Search/MediaSearch` は、0件のときだけ助詞・冠詞を除き後ろの語から減らして最大3回探し直す(`archive_org::search_relaxed`、試験 51 件成功、VPS 反映済み)。拡張機能 v0.1.1 の既定の検索語 `lo-fi piano` は実測で 3 件(全て再生可能)。
**English**: When `Search/MediaSearch` finds nothing, it retries up to 3 times with stop words removed and trailing words dropped (`archive_org::search_relaxed`; 51 tests pass; deployed). The extension v0.1.1's default prompt `lo-fi piano` measured 3 playable hits.

## Versionless API の方針 / Versionless API policy (2026-11-25)
**日本語**: このリポジトリのAPIは **Versionless**(URL・パッケージ名に `v1` 等を付けず、互換性を壊さず足すだけで進化させる)。欄は足すだけ・足す欄は省略可能・知らない欄は無視・名前変更は新名を足して旧を非推奨に(期限は既定で6か月以上、使用0を確認してから削除)・enumは未知の値を許容・安全上の緊急修正は例外。既存の `.v1` は呼び出し側を壊さないよう当面残し、版なしの別名を追加して段階的に畳む。詳細・出典は [`runo/API-POLICY.md`](https://github.com/aon-co-jp/runo/blob/main/API-POLICY.md) 末尾。適用はこれから順に行う。
**English**: This repository's APIs are **Versionless**: no `v1` in URLs or package names; evolve by adding only, without breaking compatibility. Only add fields, new fields are optional, ignore unknown fields, rename by adding the new name and deprecating the old (default deadline 6 months or more, remove only after confirming zero use), tolerate unknown enum values, emergency security fixes are the exception. Existing `.v1` names stay for now so callers do not break; a versionless alias is added and `.v1` is retired in stages. Details and sources: the end of [`runo/API-POLICY.md`](https://github.com/aon-co-jp/runo/blob/main/API-POLICY.md). Rollout is step by step.
