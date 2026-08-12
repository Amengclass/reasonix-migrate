# reasonix-migrate-tauri build script (Windows dev machine)
# - retries LNK1105 (Huorong sysdiag file lock) up to 12 times
# - DEBUG:NONE + DEV_DEBUG=0 reduce the number of lockable artifacts
# - ASCII-only output (avoids GBK/UTF-8 mojibake in this console)
$ErrorActionPreference = "Continue"
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
$env:RUSTFLAGS = "-C link-arg=/DEBUG:NONE"
$env:CARGO_PROFILE_DEV_DEBUG = "0"
$env:CARGO_BUILD_JOBS = "2"

$projectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location (Join-Path $projectRoot "src-tauri")
if (Test-Path build.log) { Remove-Item build.log -Force }

$cmd = $args[0]
if (-not $cmd) { $cmd = "build" }
if ($cmd -eq "debug") { $cmd = "build" ; $env:CARGO_BUILD_PROFILE = "dev" }
$start = Get-Date
Write-Output "===== cargo $cmd ($env:CARGO_BUILD_PROFILE) started $(Get-Date -Format 'HH:mm:ss') ====="

# custom-protocol 模式：前端产物必须先在 dist/ 里（tauri:// 协议加载它）
Write-Output "===== pnpm build:renderer ====="
& pnpm build:renderer 2>&1 | Select-Object -Last 3
if ($LASTEXITCODE -ne 0) {
    Write-Output "!!! pnpm build:renderer FAILED"
    exit 1
}

$ok = $false
for ($i = 1; $i -le 12; $i++) {
    Write-Output "===== attempt #$i ====="
    & cargo $cmd --features tauri/custom-protocol 2>&1 | Tee-Object -FilePath build.log -Append
    if ($LASTEXITCODE -eq 0) {
        $ok = $true
        Write-Output "!!! cargo $cmd SUCCESS (attempt $i)"
        break
    }
    $lnkCount = (Select-String -Path build.log -Pattern "LNK1105|閿欒浠ｇ爜 1224" | Measure-Object).Count
    if ($lnkCount -gt 0) {
        Write-Output "attempt ${i}: LNK1105 x$lnkCount (file lock), retry in 2s"
        Start-Sleep -Seconds 2
        continue
    }
    $realErr = Select-String -Path build.log -Pattern "error\[|error:" | Select-Object -Last 1
    if ($realErr) {
        Write-Output ""
        Write-Output "========== COMPILE ERROR (full log: build.log) =========="
        $skip = [Math]::Max(0, $realErr.LineNumber - 6)
        Get-Content build.log | Select-Object -Skip $skip -First 12
        Write-Output "============================================================"
        break
    }
    Write-Output "attempt ${i}: build failed (no error markers), retry in 2s"
    Start-Sleep -Seconds 2
}

$elapsed = [Math]::Round(((Get-Date) - $start).TotalSeconds, 0)
Write-Output "FINAL_OK=$ok (elapsed ${elapsed}s) profile=$env:CARGO_BUILD_PROFILE"
exit $(if ($ok) { 0 } else { 1 })
