#!/usr/bin/env bash
# 休止→自動復帰の実機確認(WSL、root)。hn.algolia.com を /etc/hosts で 127.0.0.1 に向けて接続失敗(network)を起こし、
# 探りが失敗でバックオフし、hosts を戻すと即復帰することを Health で確かめる。
set -u
BIN="${1:-$HOME/asr/target/release/aruaru-search}"
PORT=4699
D=$(mktemp -d)
cp /etc/hosts /etc/hosts.bak-live
echo "127.0.0.1 hn.algolia.com" >> /etc/hosts
ARUARU_SEARCH_BIND=127.0.0.1:$PORT ARUARU_SEARCH_DATA_DIR=$D ARUARU_SEARCH_RERANK=off "$BIN" > "$D/log" 2>&1 &
PID=$!
cleanup() { kill $PID 2>/dev/null; cp /etc/hosts.bak-live /etc/hosts; rm -f /etc/hosts.bak-live; }
trap cleanup EXIT
sleep 2
call() { curl -s -X POST -H 'content-type: application/json' -d "${2:-{\}}" http://127.0.0.1:$PORT/aruaru_search.Search/$1; }
hn() { call Health | sed 's/},{/}\n{/g' | grep '"id":"hackernews"'; }
echo "== 検索(hackernews は接続失敗になる)"
call Search '{"q":"rust","hl":"en","gl":"US"}' | cut -c1-200; echo
echo "== Health (休止: network)"; hn
echo "== 25秒待つ(探りが失敗してバックオフ)"; sleep 25; hn
echo "== hosts を戻す(相手が復帰)"; cp /etc/hosts.bak-live /etc/hosts
sleep 45; echo "== Health (探りで復帰したはず)"; hn
echo "== ログ"; grep hackernews "$D/log" | tail -8
