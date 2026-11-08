#Requires -RunAsAdministrator
<#
  WinTune - reversible debloat for this machine.

    wintune.ps1 -Mode Report    show current state + recent boot times (changes nothing)
    wintune.ps1 -Mode Apply     record original state in backup.json, then apply
    wintune.ps1 -Mode Restore   put back exactly what backup.json recorded

  Apply can be re-run after adding targets: items already in backup.json keep
  their ORIGINAL recorded state; only new targets are recorded.

  Nothing is uninstalled. Services go to on-demand (Manual) or Disabled,
  telemetry tasks are disabled, startup entries are disabled the same way
  Task Manager does it, and three policies turn off web search + Widgets.
  Service start types take effect at the next boot; nothing is stopped now.
  Keep this file ASCII-only (Windows PowerShell 5.1 reads BOM-less files as ANSI).
#>
param(
  [ValidateSet('Report', 'Apply', 'Restore')] [string]$Mode = 'Report'
)
$ErrorActionPreference = 'Stop'
$Root = $PSScriptRoot
$BackupPath = Join-Path $Root 'backup.json'
$LogPath = Join-Path $Root ('log-{0}-{1:yyyyMMdd-HHmmss}.txt' -f $Mode, (Get-Date))

function Log([string]$m) { Write-Host $m; Add-Content -Path $LogPath -Value $m -Encoding ascii }

# ---------------------------------------------------------------- targets

# Rarely used - start on demand instead of at every boot.
$ServicesManual = @(
  'AnyDesk', 'InputLeap', 'OVRService',
  'MSI_Center_Service', 'MSI_Case_Service', 'MSI_VoiceControl_Service',
  'HPPrintScanDoctorService', 'GamingServices', 'GamingServicesNet',
  'MySQL83', 'postgresql-x64-16', 'Bonjour Service', 'MuseAuthService',
  'Spooler'   # printing is rare: `net start spooler` before printing
)
# Not wanted at all (still reversible).
$ServicesDisabled = @('lfsvc')   # geolocation

$Tasks = @(
  '\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser',
  '\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser Exp',
  '\Microsoft\Windows\Application Experience\MareBackup',
  '\Microsoft\Windows\Autochk\Proxy',
  '\Microsoft\Windows\Customer Experience Improvement Program\Consolidator',
  '\Microsoft\Windows\Customer Experience Improvement Program\UsbCeip',
  '\Microsoft\Windows\DiskDiagnostic\Microsoft-Windows-DiskDiagnosticDataCollector',
  '\Microsoft\Windows\Feedback\Siuf\DmClient',
  '\Microsoft\Windows\Feedback\Siuf\DmClientOnScenarioDownload',
  '\Microsoft\Windows\Maps\MapsToastTask'
)

# Startup entries, disabled via StartupApproved (what Task Manager writes).
$Startup = @(
  @{ Key = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'; Name = 'Logitech Download Assistant' }
)

$Policies = @(
  # No Bing / web results in Start search.
  @{ Path = 'HKCU:\Software\Policies\Microsoft\Windows\Explorer'; Name = 'DisableSearchBoxSuggestions'; Value = 1 },
  @{ Path = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Search'; Name = 'BingSearchEnabled'; Value = 0 },
  # Widgets board (web-rendered) off.
  @{ Path = 'HKLM:\SOFTWARE\Policies\Microsoft\Dsh'; Name = 'AllowNewsAndInterests'; Value = 0 },
  # Taskbar search box hidden: Patina Launcher (Win, Win+S, Alt+Space) replaces it.
  @{ Path = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Search'; Name = 'SearchboxTaskbarMode'; Value = 0 },
  # Settings: hide the Home page (account / OneDrive / Game Pass promo cards).
  @{ Path = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\Explorer'; Name = 'SettingsPageVisibility'; Value = 'hide:home'; Type = 'String' }
)

$StartupDisabledBytes = [byte[]](3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)

# ---------------------------------------------------------------- helpers

function Get-SvcStart([string]$name) {
  $k = "HKLM:\SYSTEM\CurrentControlSet\Services\$name"
  if (-not (Test-Path $k)) { return $null }
  $p = Get-ItemProperty $k
  $start = [int]$p.Start
  $delayed = if ($p.PSObject.Properties['DelayedAutostart']) { [int]$p.DelayedAutostart } else { 0 }
  switch ($start) {
    2 { if ($delayed -eq 1) { 'delayed-auto' } else { 'auto' } }
    3 { 'demand' }
    4 { 'disabled' }
    default { "raw:$start" }
  }
}

function Set-SvcStart([string]$name, [string]$mode) {
  if ($mode -like 'raw:*') { Log "  skip $name (unusual start type $mode)"; return }
  $out = & sc.exe config "$name" start= $mode 2>&1
  if ($LASTEXITCODE -ne 0) { Log "  FAILED $name -> ${mode}: $out" } else { Log "  $name -> $mode" }
}

function Split-TaskPath([string]$full) {
  $i = $full.LastIndexOf('\')
  @{ Path = $full.Substring(0, $i + 1); Name = $full.Substring($i + 1) }
}

function Get-Task([string]$full) {
  $t = Split-TaskPath $full
  Get-ScheduledTask -TaskPath $t.Path -TaskName $t.Name -ErrorAction SilentlyContinue
}

function Get-RegValue([string]$path, [string]$name) {
  if (-not (Test-Path $path)) { return $null }
  $p = Get-ItemProperty $path
  # -NoEnumerate: a byte[] (StartupApproved) must come back whole, not unrolled.
  if ($p.PSObject.Properties[$name]) { Write-Output -NoEnumerate $p.$name; return }
  return $null
}

function Show-BootTimes {
  Log '--- Last 5 boots (Diagnostics-Performance, ms) ---'
  try {
    Get-WinEvent -FilterHashtable @{ LogName = 'Microsoft-Windows-Diagnostics-Performance/Operational'; Id = 100 } -MaxEvents 5 |
      ForEach-Object {
        $d = @{}
        ([xml]$_.ToXml()).Event.EventData.Data | ForEach-Object { $d[$_.Name] = $_.'#text' }
        Log ('  {0:yyyy-MM-dd HH:mm}  total {1,6}  to desktop {2,6}  post-boot {3,6}' -f $_.TimeCreated, $d.BootTime, $d.MainPathBootTime, $d.BootPostBootTime)
      }
  } catch { Log "  (no boot events: $($_.Exception.Message))" }
}

# ---------------------------------------------------------------- modes

if ($Mode -eq 'Report') {
  Log '--- Services ---'
  foreach ($s in $ServicesManual + $ServicesDisabled) { Log ('  {0,-28} {1}' -f $s, (Get-SvcStart $s)) }
  Log '--- Tasks ---'
  foreach ($t in $Tasks) { $x = Get-Task $t; Log ('  {0,-10} {1}' -f $(if ($x) { $x.State } else { 'missing' }), $t) }
  Log '--- Startup ---'
  foreach ($s in $Startup) {
    $v = Get-RegValue $s.Key $s.Name
    $state = if ($null -eq $v) { 'enabled (no entry)' } elseif ($v[0] -band 1) { 'disabled' } else { 'enabled' }
    Log ('  {0,-28} {1}' -f $s.Name, $state)
  }
  Log '--- Policies ---'
  foreach ($p in $Policies) { Log ('  {0} = {1}' -f $p.Name, (Get-RegValue $p.Path $p.Name)) }
  Show-BootTimes
  return
}

if ($Mode -eq 'Apply') {
  # 1. Record the original state of every target not already in backup.json.
  $backup = [ordered]@{ created = (Get-Date).ToString('s'); services = @(); tasks = @(); startup = @(); policies = @() }
  if (Test-Path $BackupPath) {
    $old = Get-Content $BackupPath -Raw | ConvertFrom-Json
    $backup.created = $old.created
    foreach ($k in 'services', 'tasks', 'startup', 'policies') { $backup[$k] = @($old.$k | Where-Object { $_ }) }
  }
  $added = 0
  foreach ($s in $ServicesManual + $ServicesDisabled) {
    if ($backup.services | Where-Object { $_.name -eq $s }) { continue }
    $m = Get-SvcStart $s
    if ($m) { $backup.services += @{ name = $s; start = $m }; $added++ }
  }
  foreach ($t in $Tasks) {
    if ($backup.tasks | Where-Object { $_.task -eq $t }) { continue }
    $x = Get-Task $t
    if ($x) { $backup.tasks += @{ task = $t; enabled = ($x.State -ne 'Disabled') }; $added++ }
  }
  foreach ($s in $Startup) {
    if ($backup.startup | Where-Object { $_.key -eq $s.Key -and $_.name -eq $s.Name }) { continue }
    $v = Get-RegValue $s.Key $s.Name
    $backup.startup += @{ key = $s.Key; name = $s.Name; existed = ($null -ne $v); bytes = $(if ($v) { [Convert]::ToBase64String($v) } else { $null }) }
    $added++
  }
  foreach ($p in $Policies) {
    if ($backup.policies | Where-Object { $_.path -eq $p.Path -and $_.name -eq $p.Name }) { continue }
    $v = Get-RegValue $p.Path $p.Name
    $kind = if ($null -ne $v) { (Get-Item $p.Path).GetValueKind($p.Name).ToString() } else { $null }
    $backup.policies += @{ path = $p.Path; name = $p.Name; existed = ($null -ne $v); value = $v; kind = $kind }
    $added++
  }
  $backup | ConvertTo-Json -Depth 5 | Set-Content -Path $BackupPath -Encoding ascii
  Log "Recorded $added new item(s) in $BackupPath (earlier entries keep their original state)"
  Show-BootTimes

  # 2. Apply.
  Log '--- Services ---'
  foreach ($s in $ServicesManual) { if (Get-SvcStart $s) { Set-SvcStart $s 'demand' } else { Log "  $s not installed" } }
  foreach ($s in $ServicesDisabled) { if (Get-SvcStart $s) { Set-SvcStart $s 'disabled' } else { Log "  $s not installed" } }

  Log '--- Tasks ---'
  foreach ($t in $Tasks) {
    $p = Split-TaskPath $t
    try { Disable-ScheduledTask -TaskPath $p.Path -TaskName $p.Name -ErrorAction Stop | Out-Null; Log "  disabled $t" }
    catch { Log "  FAILED $t : $($_.Exception.Message)" }
  }

  Log '--- Startup ---'
  foreach ($s in $Startup) {
    if (-not (Test-Path $s.Key)) { New-Item -Path $s.Key -Force | Out-Null }
    Set-ItemProperty -Path $s.Key -Name $s.Name -Value $StartupDisabledBytes -Type Binary
    Log "  disabled $($s.Name)"
  }

  Log '--- Policies ---'
  foreach ($p in $Policies) {
    if (-not (Test-Path $p.Path)) { New-Item -Path $p.Path -Force | Out-Null }
    Set-ItemProperty -Path $p.Path -Name $p.Name -Value $p.Value -Type $(if ($p.Type) { $p.Type } else { 'DWord' })
    Log "  $($p.Name) = $($p.Value)"
  }
  Log 'Done. Reboot for service changes; sign out/in (or restart Explorer) for search + Widgets.'
  return
}

if ($Mode -eq 'Restore') {
  if (-not (Test-Path $BackupPath)) { throw "No backup.json - nothing to restore." }
  $b = Get-Content $BackupPath -Raw | ConvertFrom-Json
  Log "Restoring state saved $($b.created)"

  Log '--- Services ---'
  foreach ($s in $b.services) { Set-SvcStart $s.name $s.start }

  Log '--- Tasks ---'
  foreach ($t in $b.tasks) {
    $p = Split-TaskPath $t.task
    try {
      if ($t.enabled) { Enable-ScheduledTask -TaskPath $p.Path -TaskName $p.Name -ErrorAction Stop | Out-Null; Log "  enabled $($t.task)" }
      else { Disable-ScheduledTask -TaskPath $p.Path -TaskName $p.Name -ErrorAction Stop | Out-Null; Log "  disabled $($t.task)" }
    } catch { Log "  FAILED $($t.task) : $($_.Exception.Message)" }
  }

  Log '--- Startup ---'
  foreach ($s in $b.startup) {
    if ($s.existed) { Set-ItemProperty -Path $s.key -Name $s.name -Value ([Convert]::FromBase64String($s.bytes)) -Type Binary }
    else { Remove-ItemProperty -Path $s.key -Name $s.name -ErrorAction SilentlyContinue }
    Log "  restored $($s.name)"
  }

  Log '--- Policies ---'
  foreach ($p in $b.policies) {
    if ($p.existed) { Set-ItemProperty -Path $p.path -Name $p.name -Value $p.value -Type $(if ($p.kind) { $p.kind } else { 'DWord' }) }
    else { Remove-ItemProperty -Path $p.path -Name $p.name -ErrorAction SilentlyContinue }
    Log "  restored $($p.name)"
  }

  $done = $BackupPath -replace '\.json$', ('-restored-{0:yyyyMMdd-HHmmss}.json' -f (Get-Date))
  Move-Item $BackupPath $done
  Log "Done. Backup moved to $done. Reboot to finish."
}
