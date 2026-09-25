# deps.lock に固定されたコミットを .deps\<名前> に取得する(Windows 用、fetch-deps.sh と同じ動作)。
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
New-Item -ItemType Directory -Force (Join-Path $root '.deps') | Out-Null
foreach ($line in Get-Content (Join-Path $root 'deps.lock') -Encoding utf8) {
    if ($line -match '^\s*(#|$)') { continue }
    $name, $url, $rev = -split $line
    $dir = Join-Path $root ".deps\$name"
    $fresh = -not (Test-Path (Join-Path $dir '.git'))
    if ($fresh) {
        Write-Host "取得: $name @ $($rev.Substring(0,12))"
        git clone --quiet --filter=blob:none --no-checkout $url $dir
        if ($LASTEXITCODE -ne 0) { throw "clone 失敗: $name" }
    }
    if (-not $fresh -and (git -C $dir status --porcelain)) {
        throw "停止: .deps\$name に変更があります。確認してから削除して再実行してください。"
    }
    $head = git -C $dir rev-parse HEAD 2>$null
    if ($fresh -or $head -ne $rev) {
        git -C $dir fetch --quiet origin $rev 2>$null
        if ($LASTEXITCODE -ne 0) { git -C $dir fetch --quiet origin }
        git -C $dir -c advice.detachedHead=false checkout --quiet $rev
        if ($LASTEXITCODE -ne 0) { throw "checkout 失敗: $name @ $rev" }
        Write-Host "固定: $name @ $($rev.Substring(0,12))"
    }
}
Write-Host "依存リポジトリは deps.lock どおりです。"
