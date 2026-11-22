param(
  [string]$From = "",     # ex: E2-L1 (retoma a partir desse lote)
  [string]$Only = "",     # ex: E1-L3 (roda só esse lote)
  [int]$MaxTurns = 60,    # turnos por sessão
  [int]$MaxFix = 3,       # tentativas de correção de gate por lote
  [switch]$List           # só lista o plano
)
$ErrorActionPreference = "Continue"
$Auto = $PSScriptRoot
$Root = (Resolve-Path (Join-Path $Auto "..\..")).Path
$App  = Join-Path $Root "apps\prismview-studio"
$Logs = Join-Path $Auto "logs"
$Done = Join-Path $Auto "done.txt"
$GateScript = Join-Path $Auto "gate.ps1"
$Handoff = Join-Path $Auto "HANDOFF.md"
New-Item -ItemType Directory -Force $Logs | Out-Null
if (-not (Test-Path $Done)) { New-Item $Done -ItemType File | Out-Null }
Set-Location $Root

# O modelo só pode validar via gate.ps1 (saída condensada) e não lê logs/lixo.
$Allowed = "Read,Edit,Write,Glob,Grep,Bash(tools/autorun/gate.cmd:*),Bash(git status:*),Bash(git diff:*)"
$Denied  = "Read(./tools/autorun/logs/**),Read(./target/**),Read(./**/node_modules/**),Read(./**/dist/**),Read(./.git/**)"
$Exclude = @(":!tools/autorun/logs", ":!tools/autorun/done.txt", ":!tools/autorun/gate-errors.txt", ":!tools/autorun/baseline-ts.txt", ":!tools/autorun/baseline-rust.txt")

$rows = New-Object System.Collections.Generic.List[string]
$total = 0.0

function Write-Report {
  $f = Join-Path $Logs "REPORT.md"
  (@("# Autorun report", "", "| lote | status | correcoes | custo |", "|---|---|---|---|") + $rows + @("", ("total ~`${0:N2}" -f $total))) | Set-Content $f
}
function Stop-Run($msg) { Write-Host "PARADO: $msg" -ForegroundColor Red; Write-Report; exit 1 }

function Get-Meta($file) {
  $m = @{}
  foreach ($line in Get-Content $file) {
    if ($line.Trim() -eq "---") { break }
    if ($line -match '^(\w+):\s*(.+)$') { $m[$Matches[1]] = $Matches[2].Trim() }
  }
  return $m
}

# Sessão claude com retry em erro transitório (rate limit, overload, rede).
function Invoke-Claude($model, $effort, $prompt, $logName) {
  for ($try = 1; $try -le 3; $try++) {
    $res = $prompt | & claude -p --model $model --effort $effort --max-turns $MaxTurns `
      --permission-mode acceptEdits --allowedTools $Allowed --disallowedTools $Denied --output-format json 2>&1
    $code = $LASTEXITCODE
    $txt = ($res | ForEach-Object { "$_" }) -join "`n"
    $txt | Out-File (Join-Path $Logs "$logName.try$try.json") -Encoding utf8
    $j = $null; try { $j = $txt | ConvertFrom-Json } catch {}
    $cost = 0.0; $sub = ""; $isErr = ($code -ne 0)
    if ($j) { try { $cost = [double]$j.total_cost_usd } catch {}; $sub = "$($j.subtype)"; if ($j.is_error) { $isErr = $true } }
    $transient = $isErr -and ($txt -match '(?i)rate.?limit|overloaded|\b429\b|\b529\b|ECONNRESET|ETIMEDOUT|timed? ?out|temporarily')
    if ($transient -and $try -lt 3) {
      $wait = 60 * $try
      Write-Host "erro transitorio; aguardando ${wait}s (tentativa $try/3)" -ForegroundColor Yellow
      Start-Sleep -Seconds $wait
      continue
    }
    return @{ ok = (-not $isErr); cost = $cost; subtype = $sub }
  }
}

function Invoke-Gate($gate) {
  & powershell -NoProfile -ExecutionPolicy Bypass -File $GateScript -Gate $gate 2>&1 | Out-Null
  $ok = ($LASTEXITCODE -eq 0)
  $count = 0
  $ef = Join-Path $Auto "gate-errors.txt"
  if (Test-Path $ef) { $first = (Get-Content $ef -TotalCount 1); if ($first -match 'NEW_ERRORS=(\d+)') { $count = [int]$Matches[1] } }
  return @{ ok = $ok; count = $count }
}

function Save-Failed($id) {
  git stash push -u -m "autorun-fail-$id" -- . @($Exclude) *> $null
  Write-Host "trabalho parcial guardado em 'git stash' (autorun-fail-$id); arvore limpa para retomar com -From $id" -ForegroundColor Yellow
}

$batches = Get-ChildItem (Join-Path $Auto "batches") -Filter *.md | Sort-Object Name

if ($List) {
  foreach ($b in $batches) { $m = Get-Meta $b.FullName; "{0,-30} {1,-7} {2,-7} {3}" -f $b.BaseName, $m.model, $m.effort, $m.gate }
  exit 0
}

# --- preparação ---
if (-not (Test-Path (Join-Path $App "node_modules"))) { Stop-Run "rode 'npm install' em apps\prismview-studio antes (sem isso o baseline seria falso)" }
$branch = "autorun/sala3d"
$cur = (git rev-parse --abbrev-ref HEAD).Trim()
if ($cur -ne $branch) {
  git rev-parse --verify $branch *> $null
  if ($LASTEXITCODE -eq 0) { git checkout $branch } else { git checkout -b $branch }
}
foreach ($k in @("ts", "rust")) {
  if (-not (Test-Path (Join-Path $Auto "baseline-$k.txt"))) {
    Write-Host "gerando baseline de erros pre-existentes ($k)..." -ForegroundColor Cyan
    & powershell -NoProfile -ExecutionPolicy Bypass -File $GateScript -Gate $k -Baseline
  }
}

$started = ($From -eq "")
foreach ($b in $batches) {
  $name = $b.BaseName
  $id = ($name -split "-")[0..1] -join "-"
  if (-not $started) { if ($id -eq $From) { $started = $true } else { continue } }
  if ($Only -ne "" -and $id -ne $Only) { continue }
  if ((Get-Content $Done) -contains $id) { Write-Host "pulando $id (feito)"; continue }

  $m = Get-Meta $b.FullName
  $gate = $m.gate
  $gateCmd = "tools/autorun/gate.cmd $gate"
  Write-Host "=== $name | $($m.model)/$($m.effort) | gate=$gate ===" -ForegroundColor Cyan
  $handoffBefore = (Get-Item $Handoff).LastWriteTimeUtc

  $prompt = @"
Execute SOMENTE o lote $id. Leia, nesta ordem: CLAUDE.md, tools/autorun/CONTEXT.md, tools/autorun/HANDOFF.md, tools/autorun/batches/$($b.Name).
Regras:
- Implemente tudo que o lote pede, de ponta a ponta. Proibido: TODO, placeholder, mock, stub, adiar, gambiarra no lugar da solucao correta.
- Leia so o necessario (grep por simbolo e leia janelas curtas). Patches pontuais (Edit); nunca reescreva arquivo existente inteiro.
- Nao implemente nada de outros lotes. Se faltar pre-requisito, escreva 'BLOCKED: motivo' no HANDOFF.md e termine.
- Validacao: rode SOMENTE ``$gateCmd`` (saida ja condensada, so erros novos). Nunca rode npm run build / cargo check diretos e nunca leia logs brutos ou tools/autorun/logs.
- Se o gate acusar erro, corrija a causa raiz e rode o gate de novo, ate zerar. Proibido silenciar erro com @ts-ignore, 'as any' em massa, #[allow] ou removendo funcionalidade.
- Por ultimo, SOBRESCREVA tools/autorun/HANDOFF.md com no maximo 10 linhas: o que mudou (arquivos/simbolos), decisoes, o que o proximo lote precisa saber.
"@

  $r = Invoke-Claude $m.model $m.effort $prompt $name
  $total += $r.cost

  # sessão estourou turnos: continua em sessão nova, com o estado do git (até 2x)
  $cont = 0
  while ($r.subtype -eq "error_max_turns" -and $cont -lt 2) {
    $cont++
    Write-Host "limite de turnos em $id; continuacao $cont/2" -ForegroundColor Yellow
    $stat = (git diff --stat | Select-Object -Last 15) -join "`n"
    $p2 = "Continue o lote $id (a sessao anterior atingiu o limite de turnos). Estado (git diff --stat):`n$stat`nLeia CLAUDE.md, tools/autorun/CONTEXT.md e tools/autorun/batches/$($b.Name). Confira o que falta com grep/janelas curtas, termine o lote, valide com ``$gateCmd`` e atualize HANDOFF.md (max 10 linhas)."
    $r = Invoke-Claude $m.model $m.effort $p2 "$name.cont$cont"
    $total += $r.cost
  }
  if (-not $r.ok) { $rows.Add("| $id | FALHOU (claude) | - | - |"); Save-Failed $id; Stop-Run "sessao claude falhou em $id (veja logs\$name.try*.json)" }
  if ((Get-Content $Handoff -Raw) -match "BLOCKED") { $rows.Add("| $id | BLOQUEADO | - | - |"); Save-Failed $id; Stop-Run "lote $id bloqueado (veja HANDOFF.md no stash/arvore)" }

  # gate autoritativo (do script), com correção escalonada
  $g = Invoke-Gate $gate
  $k = 0; $prevCount = $g.count
  while (-not $g.ok -and $k -lt $MaxFix) {
    $k++
    if ($k -eq 1) { $fm = "sonnet"; $fe = "high" } else { $fm = "opus"; $fe = "medium" }
    Write-Host "gate falhou em $id ($($g.count) erros novos); correcao $k/$MaxFix com $fm/$fe" -ForegroundColor Yellow
    $stat = (git diff --stat | Select-Object -Last 12) -join "`n"
    $fix = @"
O gate '$gate' do lote $id falhou com $($g.count) erros NOVOS (correcao $k/$MaxFix).
Leia SOMENTE tools/autorun/gate-errors.txt (ja condensado) e, para cada erro, uma janela de ~40 linhas ao redor (Read com offset/limit). Arquivos do lote (git diff --stat):
$stat
Corrija a causa raiz com patches pontuais, sem mudar o escopo. Proibido silenciar com @ts-ignore, 'as any' em massa, #[allow], stub, ou apagar funcionalidade. Se o erro vier de tipo/contrato desalinhado entre arquivos, alinhe pelo CONTRATO FINAL em tools/autorun/CONTEXT.md.
Revalide com ``$gateCmd`` (unico comando de build permitido). Nunca leia logs brutos.
"@
    $r2 = Invoke-Claude $fm $fe $fix "$name.fix$k"
    $total += $r2.cost
    $g = Invoke-Gate $gate
    if (-not $g.ok -and $k -ge 2 -and $g.count -ge $prevCount) { Write-Host "sem progresso nas correcoes de $id" -ForegroundColor Yellow; break }
    $prevCount = $g.count
  }
  if (-not $g.ok) {
    $rows.Add("| $id | GATE FALHOU ($($g.count) erros) | $k | ~`${0:N2} |" -f $total)
    Save-Failed $id
    Stop-Run "gate '$gate' ainda falha em $id; veja tools/autorun/gate-errors.txt"
  }

  # handoff garantido mesmo se o modelo esquecer
  if ((Get-Item $Handoff).LastWriteTimeUtc -eq $handoffBefore) {
    $stat = (git diff --stat | Select-Object -Last 8) -join "`n"
    "AUTO: lote $id concluido sem handoff do modelo. Arquivos:`n$stat" | Set-Content $Handoff
  }

  git add -A -- . @($Exclude)
  git commit -qm "autorun ${id}: $($b.BaseName)" 2>&1 | Out-Null
  Add-Content $Done $id
  $rows.Add("| $id | ok | $k | ~`$$([math]::Round($total,2)) acum. |")
  Write-Host ("ok {0} | custo acumulado ~`${1:N2}" -f $id, $total) -ForegroundColor Green
}
Write-Report
Write-Host "FIM. Relatorio em tools\autorun\logs\REPORT.md (custo total ~`$$([math]::Round($total,2)))" -ForegroundColor Green
