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
curl 'http://127.0.0.1:4610/v1/search?q=%E5%B1%B1%E6%A2%A8%E7%9C%8C+%E6%B8%A9%E6%B3%89&hl=ja&n=10'
```

| API | 内容 |
|---|---|
| `GET /v1/search?q=&hl=&gl=&n=` / `POST /v1/search` | 検索。`hl` は言語(ja, zh-TW, ar, fa …)、`gl` は地域(省略可)。`{"results":[{title,link,snippet,engines,score}],"warnings":[]}` |
| `GET /v1/engines` | 検索元の設定と最近の状態 |
| `POST /admin/selfcheck` | 点検と自動保守を今すぐ実行(`x-admin-token`。`ARUARU_SEARCH_ADMIN_TOKEN` 設定時のみ) |
| `GET /healthz` | 死活確認 |

環境変数: `ARUARU_SEARCH_BIND`(既定 `127.0.0.1:4610`)、`ARUARU_SEARCH_DATA_DIR`(既定 `data`)、
`ARUARU_LLM_URL`(既定 `http://127.0.0.1:4600`)、`ARUARU_SEARCH_ADMIN_TOKEN`。

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
