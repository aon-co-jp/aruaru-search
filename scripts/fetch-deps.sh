#!/usr/bin/env bash
# deps.lock に固定されたコミットを .deps/<名前> に取得する(冪等)。
# 既存の .deps/<名前> がロックと異なるコミットなら、そのコミットへ切り替える。
# .deps/ 内で手作業の変更があれば上書きせずに停止する。
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/.deps"
grep -vE '^\s*(#|$)' "$root/deps.lock" | while read -r name url rev; do
  dir="$root/.deps/$name"
  fresh=0
  if [ ! -d "$dir/.git" ]; then
    fresh=1
    echo "取得: $name @ ${rev:0:12}"
    git clone --quiet --filter=blob:none --no-checkout "$url" "$dir"
  fi
  if [ "$fresh" = 0 ] && [ -n "$(git -C "$dir" status --porcelain 2>/dev/null)" ]; then
    echo "停止: .deps/$name に変更があります。確認してから削除して再実行してください。" >&2
    exit 1
  fi
  if [ "$fresh" = 1 ] || [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null || true)" != "$rev" ]; then
    git -C "$dir" fetch --quiet origin "$rev" 2>/dev/null || git -C "$dir" fetch --quiet origin
    git -C "$dir" -c advice.detachedHead=false checkout --quiet "$rev"
    echo "固定: $name @ ${rev:0:12}"
  fi
done
echo "依存リポジトリは deps.lock どおりです。"
