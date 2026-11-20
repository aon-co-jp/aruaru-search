
## 2026-11-04 依存更新 / Dependency update
- reqwest 0.12→0.13（TLS機能名は`rustls-tls`→`rustls`）。ビルド・テスト44件成功。`.deps/`配下の他リポジトリ由来の遅れ（bincode/russh/rand等）と、修正版の無いrsa（RUSTSEC-2023-0071）は各リポジトリ側の対応待ち。
- reqwest 0.12→0.13 (TLS feature renamed `rustls-tls`→`rustls`). Build and 44 tests pass. Gaps inside `.deps/` (bincode, russh, rand...) and rsa (RUSTSEC-2023-0071, no fix) belong to the upstream repos.

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

## 2026-11-04 REST から Connect へ / REST → Connect
- 新設 `src/connect.rs`（`aruaru_search.v1.Search/{Search,MediaSearch,Engines,Health}`, `Admin/SelfCheck`）。JSON は RS-JSON（`from_str_secure`、64KiB・深さ・重複キー上限、`rust_json::fast`）。`deps.lock` に RS-JSON を追加。`/v1/health`・`/v1/engines`・`/admin/selfcheck` は削除。`/v1/search`・`/v1/media-search` は呼び出し側（aruaru-llm `web_search.rs`、open-english `news_qa.rs`、open-upstream-watch `services.rs`）の移行まで互換で残す（移行後に削除）。画面は Connect 呼び出しへ。
- Added `src/connect.rs` (Connect unary API); removed `/v1/health`, `/v1/engines`, `/admin/selfcheck`; `/v1/search` and `/v1/media-search` stay as time-limited compat until the three callers migrate. UI now calls Connect.
- **デプロイ注意**: VPS 反映で `/v1/health` 等を叩く監視があれば `Search/Health` へ。
- 追記(同日): 呼び出し側3つ(aruaru-llm / open-english / open-upstream-watch)をConnectへ移し、`/v1/search`・`/v1/media-search`も削除。**デプロイ順: 先にaruaru-search、その後に呼び出し側**。

- 追記(2026-11-05): **VPS 反映済み**(`38d6e3a`)。呼び出し元の洗い出しの結果、未移行だった realdata.pro(`schema.rs` の `/v1/health` → `Search/Health`)と `audiocafe-tokyo-rust` の `diet_crawl.py`(`/v1/search` → `Search/Search`)も Connect(POST、RS-JSON)へ。aruaru-llm への呼び出しは `Llm/Rerank`・`Llm/ChatProvidersCompletePriority`。外部から来る JSON(archive.org・検索元API・aruaru-llm の応答)は `from_str_secure` で読む。
- Added (2026-11-05): deployed to the VPS (`38d6e3a`); remaining callers (realdata.pro health, diet_crawl.py) moved to Connect (POST, RS-JSON); external JSON is read with `from_str_secure`.

## 2026-11-06 休止からの自動再開 / Auto-resume from cooldown
- 実装は `src/search.rs`(`BlockKind`・`probe_engine`・`probe_due`・`Searcher::with_clock`)と `src/main.rs`(10秒ごとの探り)。設計・従来ロジックの調査結果は `CLAUDE.md`「休止からの自動再開」。
- 試験: `cargo test`(偽の時計+手元の模擬サーバーで、即復帰・指数バックオフ・理由の区別・別の検索元への切り替え)。
- 本番反映: Ubuntu-24.04(WSL、glibc 2.39)でリリースビルド → scp → `.bak-日付` を残して入れ替え → `systemctl restart aruaru-search` → `Search/Health` で確認。VPS ではビルドしない(メモリ 3.6GB)。
- English: cooled-down engines are now probed with a fixed harmless query when due; success recovers immediately, failure backs off exponentially (cooldown end is never extended). Reasons (rejected / rate_limited / network) get different cooldown lengths. `Health` shows `cooldown_reason`, `next_probe_secs`, `last_probe`.
