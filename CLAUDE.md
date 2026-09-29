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
いないことを確認済み。VPS で実機確認済み(`/v1/health` で `excite` が healthy、実クエリの
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
- 無効化した経緯は `maintenance.log` に残り、**再度使うには手動で `enabled` を true に戻す**
  必要がある(AI が自分で再度有効化することはできない設計のまま)。
- `EngineDef.give_up_streak`(`engine.rs`)・`RepairOutcome`/`GiveUp`/`bump_give_up_streak`/
  `reset_give_up_streak`(`maintain.rs`)で実装。テスト(`give_up_streak_disables_after_threshold_and_resets_on_recovery`
  など)で、しきい値未満では無効化しないこと・しきい値到達で無効化されること・回復で streak が
  0に戻ることを確認済み。

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
