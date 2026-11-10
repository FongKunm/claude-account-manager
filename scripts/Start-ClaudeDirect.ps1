# Standalone Windows PowerShell 5.1 / PowerShell 7. No account-manager dependency.
[CmdletBinding(DefaultParameterSetName='Direct')]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Account,
    [Parameter(Mandatory,ParameterSetName='Direct')][ValidateRange(1,65535)][int]$Port,
    [Parameter(Mandatory,ParameterSetName='Direct')][ipaddress]$ExpectedIP,
    [Parameter(Mandatory,ParameterSetName='Saved')][switch]$FromSavedCard,
    [ValidateSet('Login','Status','Chat')][string]$Action = 'Chat',
    [string]$Transcript,
    [Parameter(ParameterSetName='Direct')][switch]$UseDefaultLogin,
    [switch]$FullAccess
)
$ErrorActionPreference = 'Stop'
function Read-LocalJson([string]$File) {
    $json = [IO.File]::ReadAllText($File)
    if ($PSVersionTable.PSVersion.Major -ge 6) { return ($json | ConvertFrom-Json -AsHashtable) }
    # Native project path keys can differ only by case; the standard PS 5.1
    # ConvertFrom-Json turns them into conflicting object properties.
    Add-Type -AssemblyName System.Web.Extensions
    $parser = New-Object System.Web.Script.Serialization.JavaScriptSerializer
    $parser.MaxJsonLength = [int]::MaxValue
    return $parser.DeserializeObject($json)
}
$claude = Join-Path $env:USERPROFILE '.local\bin\claude.exe'
if (-not (Test-Path -LiteralPath $claude -PathType Leaf)) {
    $claude = (Get-Command claude.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
}
if ($UseDefaultLogin -and $Action -eq 'Login') { throw 'The default login already exists. Use Status or Chat.' }
if ($Transcript) {
    $Transcript = (Get-Item -LiteralPath $Transcript -ErrorAction Stop).FullName
    if ($Action -ne 'Chat' -or [IO.Path]::GetExtension($Transcript) -ne '.jsonl') { throw 'Resume requires one closed Claude .jsonl transcript and the Chat action.' }
}
$base = Join-Path $env:LOCALAPPDATA 'ClaudeDirect'
if ($FromSavedCard) {
    if ($Action -eq 'Login') { throw 'This card is already enrolled. Use Status or Chat.' }
    $store = Join-Path $env:LOCALAPPDATA 'ClaudeAccountManager'
    $cards = (Read-LocalJson (Join-Path $store 'accounts.json')).ordinaryAccounts
    $matchesByName = @($cards | Where-Object { $_.name -eq $Account -or $_.id -eq $Account })
    if ($matchesByName.Count -ne 1) { throw 'Select one enrolled ordinary account by its exact unique name or login-ID.' }
    $card = $matchesByName[0]
    if ($card.id -notmatch '^login-([0-9a-f-]{36})$') { throw 'Invalid registered account ID.' }
    $id = ([guid]$Matches[1]).ToString('D')
    $slotName = "saved-$id"
    $configDir = Join-Path $store "ordinary-logins\$id\.claude"
    $nativeAccount = (Read-LocalJson (Join-Path $configDir '.claude.json')).oauthAccount
    if (-not $nativeAccount.accountUuid -or $nativeAccount.accountUuid -ne $card.accountUuid -or $nativeAccount.organizationUuid -ne $card.org) { throw 'Registered account and native profile do not match.' }
    $listener = [uri]$card.proxy
    if ($listener.Scheme -ne 'http' -or -not $listener.IsLoopback -or $listener.UserInfo -or $listener.PathAndQuery -ne '/' -or $listener.Fragment) { throw 'A fixed local HTTP proxy is required.' }
    $Port = $listener.Port
    $ExpectedIP = [ipaddress]$card.expectedIp
    $proxy = $card.proxy
} else {
    if ($Account -notmatch '^[A-Za-z0-9][A-Za-z0-9_-]{0,39}$' -or $Account -like 'saved-*') { throw 'Use a simple local name such as A or B.' }
    $slotName = $Account
    $proxy = "http://127.0.0.1:$Port"
    $configDir = if ($UseDefaultLogin) { Join-Path $env:USERPROFILE '.claude' } else { Join-Path (Join-Path $base $Account) 'config' }
}
$slot = Join-Path $base $slotName
$binding = Join-Path $slot 'binding.json'
$globalFile = if ($UseDefaultLogin) { Join-Path $env:USERPROFILE '.claude.json' } else { Join-Path $configDir '.claude.json' }
if ($env:CLAUDE_CODE_FORCE_WINDOWS_CREDMAN -eq '1' -or (Test-Path -LiteralPath (Join-Path $configDir '.config.json'))) { throw 'This example requires native file storage and the current Claude configuration layout.' }
if (Test-Path -LiteralPath $globalFile) {
    $nativeConfig = Read-LocalJson $globalFile
    if ($nativeConfig.cachedGrowthBookFeatures.tengu_windows_credman -eq $true) { throw 'Windows Credential Manager is outside the scope of this file-storage example.' }
}
function Check-Bindings {
    foreach ($dir in Get-ChildItem -LiteralPath $base -Directory) {
        $file = Join-Path $dir.FullName 'binding.json'
        if (-not (Test-Path -LiteralPath $file)) { continue }
        $saved = Get-Content -LiteralPath $file -Raw | ConvertFrom-Json
        $sameIp = ([ipaddress]$saved.ip).Equals($ExpectedIP)
        if ($dir.Name -eq $slotName) {
            if ($saved.port -ne $Port -or -not $sameIp -or $saved.defaultLogin -ne [bool]$UseDefaultLogin) { throw 'This account already has a different binding. Claude was not started.' }
        } elseif ($saved.port -eq $Port -or $sameIp -or ($UseDefaultLogin -and $saved.defaultLogin)) {
            throw "That port, IP or default login already belongs to $($dir.Name). Claude was not started."
        }
    }
}
# curl.exe ships with Windows. -q ignores .curlrc; remote hosts cannot bypass this proxy.
$curl = (Get-Command curl.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$curlArgs = @('-q','--fail','--silent','--show-error','--proto','=https','--connect-timeout','5','--max-time','12','--proxy',$proxy,'--noproxy','localhost,127.0.0.1,::1')
if ($env:CURL_CA_BUNDLE) { $curlArgs += @('--cacert',$env:CURL_CA_BUNDLE) }
foreach ($hostName in @('claude.ai','api.anthropic.com')) {
    $trace = & $curl @curlArgs "https://$hostName/cdn-cgi/trace"
    $exitCode = $LASTEXITCODE
    $ipLines = @($trace | Where-Object { $_ -match '^ip=' })
    if ($exitCode -ne 0 -or $ipLines.Count -ne 1) { throw "Unable to verify $hostName. Claude was not started." }
    $seenIp = [ipaddress]($ipLines[0].Substring(3).Trim())
    if (-not $seenIp.Equals($ExpectedIP)) { throw "$hostName showed $seenIp, expected $ExpectedIP. Claude was not started." }
}
[IO.Directory]::CreateDirectory($base) | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
$gate = [IO.File]::Open((Join-Path $base 'bindings.lock'),'OpenOrCreate','ReadWrite','None')
try {
    Check-Bindings
    [IO.Directory]::CreateDirectory($slot) | Out-Null
    if (-not $UseDefaultLogin -and -not $FromSavedCard) { [IO.Directory]::CreateDirectory($configDir) | Out-Null }
    if (-not (Test-Path -LiteralPath $binding)) {
        [IO.File]::WriteAllText($binding,(@{port=$Port;ip=$ExpectedIP.ToString();defaultLogin=[bool]$UseDefaultLogin} | ConvertTo-Json),$utf8)
    }
    $route = @{HTTP_PROXY=$proxy;HTTPS_PROXY=$proxy;ALL_PROXY=$proxy;NO_PROXY='localhost,127.0.0.1,::1'}
    $settings = Join-Path $slot 'route-settings.json'
    $routeJson = @{env=$route} | ConvertTo-Json
    if (-not (Test-Path -LiteralPath $settings) -or [IO.File]::ReadAllText($settings) -ne $routeJson) {
        [IO.File]::WriteAllText($settings,$routeJson,$utf8)
    }
} finally {
    $gate.Dispose()
}

$start = New-Object System.Diagnostics.ProcessStartInfo
$start.FileName = $claude
$start.UseShellExecute = $false
$start.WorkingDirectory = if ($Action -eq 'Chat') { (Get-Location).ProviderPath } else { $slot }
# Start from ordinary OS variables, without inheriting another account's API key or token.
$childEnv = $start.EnvironmentVariables
$childEnv.Clear()
foreach ($key in @('SystemRoot','WINDIR','COMSPEC','PATH','PATHEXT','USERPROFILE','APPDATA','LOCALAPPDATA','TEMP','TMP','PROGRAMFILES','PROGRAMFILES(X86)','PROGRAMDATA','NUMBER_OF_PROCESSORS','PROCESSOR_ARCHITECTURE','LANG','TERM','WT_SESSION','BROWSER','CLAUDE_CODE_GIT_BASH_PATH','NODE_EXTRA_CA_CERTS')) {
    $value = [Environment]::GetEnvironmentVariable($key)
    if ($value) { $childEnv[$key] = $value }
}
if (-not $UseDefaultLogin) { $childEnv['CLAUDE_CONFIG_DIR'] = $configDir }
$childEnv['CLAUDE_SECURESTORAGE_CONFIG_DIR'] = $configDir
foreach ($entry in $route.GetEnumerator()) { $childEnv[$entry.Key] = $entry.Value }
$childEnv['CLAUDE_CODE_DISABLE_BG_EXIT_HANDOFF'] = '1'
$childEnv['CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC'] = '1'
switch ($Action) {
    'Login' { $start.Arguments = 'auth login --claudeai' }
    'Status' { $start.Arguments = 'auth status --text' }
    'Chat' {
        $start.Arguments = '--setting-sources user --settings "' + $settings + '"'
        if ($Transcript) { $start.Arguments += ' --resume "' + $Transcript + '"' }
        if ($FullAccess) { $start.Arguments += ' --dangerously-skip-permissions' }
    }
}
Write-Host "[$Account] $proxy -> $ExpectedIP | $configDir"
$process = [Diagnostics.Process]::Start($start)
try { $process.WaitForExit(); exit $process.ExitCode } finally { $process.Dispose() }
