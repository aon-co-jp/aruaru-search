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
