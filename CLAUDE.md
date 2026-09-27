# aruaru-search 開発方針

作業ドライブは `F:\aruaru-search`。共通ルールは [`open-raid-z/CLAUDE.md`](https://github.com/aon-co-jp/open-raid-z) を正本とする。

## 役割
Rust 製の自前メタ検索(APIキー不要)。aruaru-llm の検索の第一候補として、VPS で aruaru-llm とセットで動かす。

## 設計上の約束
- 検索元の違いは**コードではなく設定**(`engines.default.json` / `data/engines.json`)。ページの作りが変わったら設定(CSS セレクタ)を直す。
- AI が変えてよいのはセレクタと転送パラメータ名だけ。**URL・重み・有効/無効は AI に変えさせない**。提案は実ページで3件以上読み取れると確認してから取り込む(`maintain.rs`)。
- 英語・アメリカ中心にしない: 言語ごとの `hl/gl` 正規化・文字種による並べ替え・得意な検索元の重み付け(`lang.rs`)を崩さない。新しい言語の対応は `languages.rs`(約130言語)と `lang.rs` の表に足す。
- 検索元に負担をかけない: 間隔(`MIN_INTERVAL`)・拒否後の休止(`COOLDOWN`)・キャッシュを弱めない。
- 依存は `deps.lock` でコミット固定し `.deps/` に取得(`scripts/fetch-deps.sh`)。`cargo fmt --all` は使わず `cargo fmt-own` を使う。

## 未実施・次の候補
- 韓国(Naver)・中国(Baidu)・ロシア(Yandex)など地域の検索元の追加(実ページで検証してから)。
- (実装済み・要有効化)設定・保守履歴の GitHub 非公開リポジトリへの保存(`ARUARU_SEARCH_ARCHIVE_REPO`)。**aruaru-db・VPS のディスクには保存しない**(ユーザー指示 2026-09-25)。
- 結果の意味的な並べ替え(open-cuda 等の計算資源を使う埋め込み)。

## archive.org 横断検索(`/v1/media-search`、2026-09-27 実装・実機確認済み)

曲名・演奏者・作曲者・レーベル名で archive.org を検索し、パブリックドメイン・CC0・CC-BY・
CC-BY-SA の音源だけを即時ストリーミング可能な形(`stream_url`)で返す新エンドポイント。
既存の `engine.rs`(HTML 解析前提)には混ぜず、`src/archive_org.rs` に専用モジュールとして
実装した(archive.org は構造化 JSON API を持つため)。ライセンスの絞り込みは
`is_allowed_license()` でこちら側が厳密に行う(archive.org 自体は CC-BY-NC 等の非商用限定
音源も検索結果に含むため)。実機で `curl 'http://127.0.0.1:4610/v1/media-search?q=Caruso&n=5'`
を確認し、Enrico Caruso のパブリックドメイン録音がストリーミング URL 付きで返ることを確認済み。

- **未実施**: VPS へのデプロイ・本番反映(ローカルの実機確認のみ)。`open-bar`/`open-music-llm`
  等、実際に音源を再生・利用する側からこの API を呼ぶ導線はまだ無い。
- **未実施**: 大量アクセス時の archive.org 側への配慮(現状は個別の HTTP リクエストのみで、
  既存の検索元のような間隔調整・拒否時の休止は archive.org 向けには実装していない。
  archive.org は比較的緩やかな API だが、将来的に負荷が増えるなら追加を検討)。
