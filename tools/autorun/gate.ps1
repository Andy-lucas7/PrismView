param(
  [ValidateSet("ts", "rust", "both")][string]$Gate = "ts",
  [switch]$Baseline,      # grava baseline-<kind>.txt (rodar só em árvore limpa)
  [int]$Max = 30          # máximo de erros NOVOS exibidos
)
# Saída condensada: só erros NOVOS (vs baseline), sem warnings, sem log bruto.
# Tudo vai também para tools/autorun/gate-errors.txt. Exit 0 = sem erros novos.
$ErrorActionPreference = "Continue"
$Auto = $PSScriptRoot
$Root = (Resolve-Path (Join-Path $Auto "..\..")).Path
$App = Join-Path $Root "apps\prismview-studio"
$OutFile = Join-Path $Auto "gate-errors.txt"

function Normalize([string]$line) {
  $l = $line.Trim()
  $l = $l -replace ':\d+:\d+(?=:)', ''   # rust: arquivo:linha:col:
  $l = $l -replace '\(\d+,\d+\)', ''      # tsc: arquivo(linha,col)
  if ($l.Length -gt 300) { $l = $l.Substring(0, 300) }
  return $l
}

# retorna $null se a toolchain não existe (não validado), senão lista de @{sig;raw}
function Get-Errors([string]$kind) {
  $items = New-Object System.Collections.Generic.List[object]
  if ($kind -eq "ts") {
    Push-Location $App
    $o = @(npm run build 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    $pat = 'error TS\d+|\[vite|error during build|Could not resolve|RollupError|^error'
    if ($code -eq 0) {
      $o = @(npm test 2>&1 | ForEach-Object { "$_" })
      $code = $LASTEXITCODE
      $pat = '^\s*not ok|^✖|\bfailing\b|^# fail [1-9]|Error:'
    }
    Pop-Location
  }
  else {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { return $null }
    Push-Location $Root
    $o = @(cargo check --workspace --message-format=short --color never 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    Pop-Location
    $pat = '^\S.*: error|^error(\[|:)'
  }
  if ($code -ne 0) {
    $hits = @($o | Where-Object { $_ -match $pat -and $_ -notmatch 'could not compile|aborting due to' })
    if ($hits.Count -eq 0) { $hits = @($o | Select-Object -Last 15 | ForEach-Object { "UNPARSED: $_" }) }
    foreach ($h in $hits) { $items.Add(@{ sig = (Normalize $h); raw = $h.Trim() }) }
  }
  return , $items
}

$kinds = if ($Gate -eq "both") { @("ts", "rust") } else { @($Gate) }
$lines = New-Object System.Collections.Generic.List[string]
$totalNew = 0

foreach ($k in $kinds) {
  $base = Join-Path $Auto "baseline-$k.txt"
  $items = Get-Errors $k
  if ($null -eq $items) { $lines.Add("[$k] NAO VALIDADO: toolchain ausente (cargo nao encontrado)"); continue }

  if ($Baseline) {
    @($items | ForEach-Object { $_.sig } | Sort-Object -Unique) | Set-Content $base
    Write-Host "[$k] baseline: $(@($items).Count) erros pre-existentes registrados"
    continue
  }

  $known = @{}
  if (Test-Path $base) { Get-Content $base | ForEach-Object { $known[$_] = $true } }
  $seen = @{}
  $new = @()
  foreach ($it in $items) {
    if ($known.ContainsKey($it.sig) -or $seen.ContainsKey($it.sig)) { continue }
    $seen[$it.sig] = $true
    $new += , $it
  }
  $ignored = @($items).Count - $new.Count
  $totalNew += $new.Count
  if ($new.Count -eq 0) { $lines.Add("[$k] OK ($ignored erros pre-existentes ignorados)"); continue }
  $lines.Add("[$k] $($new.Count) erros NOVOS ($ignored pre-existentes ignorados):")
  foreach ($it in ($new | Sort-Object { $_.raw } | Select-Object -First $Max)) { $lines.Add("  " + $it.raw) }
  if ($new.Count -gt $Max) { $lines.Add("  ... +$($new.Count - $Max) omitidos (corrija os primeiros e rode o gate de novo)") }
}

if ($Baseline) { exit 0 }
$out = @("NEW_ERRORS=$totalNew") + $lines
$out | Set-Content $OutFile
$out | ForEach-Object { Write-Host $_ }
if ($totalNew -gt 0) { exit 1 } else { exit 0 }
