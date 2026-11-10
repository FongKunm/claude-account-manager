$ErrorActionPreference = 'Stop'
$camProject = Split-Path -Parent $PSScriptRoot
$camTestRoot = Join-Path ([IO.Path]::GetTempPath()) ('smoke-localappdata-' + [Guid]::NewGuid().ToString('N'))
$camOriginalLocalAppData = $env:LOCALAPPDATA
$camOriginalSmoke = $env:CAM_SMOKE_TEST
New-Item -ItemType Directory -Path $camTestRoot | Out-Null
try {
    # Child-process-only overrides. No user registry or existing terminal is changed.
    $env:LOCALAPPDATA = $camTestRoot
    $env:CAM_SMOKE_TEST = '1'
    Push-Location $camProject
    try {
        pnpm exec tauri dev --config src-tauri/smoke.conf.json --no-watch 2>&1 | Tee-Object -Variable camSmokeOutput
        $camTestExit = $LASTEXITCODE
    } finally { Pop-Location }
    if ($camTestExit -ne 0 -or -not ($camSmokeOutput -match 'CAM_SMOKE_PASS')) {
        throw '隐藏窗口集成测试失败，请查看 CAM_SMOKE 标记。'
    }
} finally {
    $env:LOCALAPPDATA = $camOriginalLocalAppData
    $env:CAM_SMOKE_TEST = $camOriginalSmoke
}
