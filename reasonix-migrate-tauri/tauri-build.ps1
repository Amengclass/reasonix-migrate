# reasonix-migrate-tauri release 构建（tauri build + LNK1105 重试）
$ErrorActionPreference = "Continue"
$env:RUSTFLAGS = "-C link-arg=/DEBUG:NONE"
$env:CARGO_PROFILE_RELEASE_DEBUG = "0"
$env:CARGO_BUILD_JOBS = "2"
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root
$start = Get-Date
Write-Output "===== tauri build started $(Get-Date -Format 'HH:mm:ss') ====="

$ok = $false
for ($i = 1; $i -le 8; $i++) {
    Write-Output "===== attempt #$i ====="
    pnpm tauri build 2>&1 | Tee-Object -FilePath "$root\tauri-build.log" -Append
    if ($LASTEXITCODE -eq 0) {
        $ok = $true
        Write-Output "!!! tauri build SUCCESS (attempt $i)"
        break
    }
    $lnk = (Select-String -Path "$root\tauri-build.log" -Pattern "LNK1105|错误代码 1224" | Measure-Object).Count
    if ($lnk -gt 0) {
        Write-Output "attempt ${i}: LNK1105 x$lnk, retry in 2s"
        Start-Sleep -Seconds 2
        continue
    }
    Write-Output "attempt ${i}: build failed without LNK1105 (see tauri-build.log)"
    break
}

$elapsed = [Math]::Round(((Get-Date) - $start).TotalSeconds, 0)
Write-Output "FINAL_OK=$ok (elapsed ${elapsed}s)"
exit $(if ($ok) { 0 } else { 1 })
