# 自检脚本（Windows）。任何一步失败就停下并返回非零。
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

function Invoke-Step([string]$name, [scriptblock]$cmd) {
    Write-Host "==> $name"
    & $cmd
    if ($LASTEXITCODE -ne 0) { throw "$name failed (exit $LASTEXITCODE)" }
}

Invoke-Step 'cargo test --release --lib' { cargo test --release --lib }
Invoke-Step 'cargo test --release --test golden' { cargo test --release --test golden }
Invoke-Step 'cargo build --release --bins' { cargo build --release --bins }

$py = $null
foreach ($c in 'python3', 'python') {
    if (-not (Get-Command $c -ErrorAction SilentlyContinue)) { continue }
    try {
        & $c -c 'import cv2, numpy' 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { $py = $c; break }
    } catch { }
}

if (-not $py) {
    Write-Host '跳过 Python 互通测试'
} else {
    $keyFile = Join-Path ([System.IO.Path]::GetTempPath()) ("bm-check-" + [guid]::NewGuid().ToString('N') + ".key")
    try {
        $hex = -join (0..31 | ForEach-Object { $_.ToString('x2') })
        Set-Content -NoNewline -Encoding ascii -Path $keyFile -Value $hex
        $prev = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $out = (& $py python/blindmark.py detect tests/golden/marked.png --key $keyFile 2>$null | Out-String)
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -ne 0) { throw "python reference detect failed (exit $code)" }
        $det = $out | ConvertFrom-Json
        if (-not $det.found -or $det.id_hex -ne '0x0badf00d') {
            throw "python reference mismatch: found=$($det.found) id=$($det.id_hex)"
        }
        Write-Host 'python interop: ok'
    } finally {
        Remove-Item $keyFile -ErrorAction SilentlyContinue
    }
}

Write-Host 'ALL CHECKS PASSED'
