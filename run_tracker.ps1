<#
  run_tracker.ps1 - bounded, supervised execution of NANOARCH_TRACKER.md

  Run from C:\Projects\NanoArch:
      powershell -ExecutionPolicy Bypass -File .\run_tracker.ps1
  Optional: -MaxRounds 10 (default 10)

  Guards:
    * at most MaxRounds rounds, one tracker item per round
    * works on a fresh branch (never on main)
    * claude runs with --permission-mode acceptEdits and an allowlist limited to cargo and git
      (git add -A, git add ., push, reset, clean, checkout are denied)
    * items that need a user decision or an external source are marked [?] up front and skipped
    * the script itself re-runs cargo test and clippy after each round; it never trusts the model
    * stops at the first failure, no progress, or dirty working tree
  This file is ASCII only on purpose (Windows PowerShell 5.1 reads BOM-less files as ANSI).
#>
param([int]$MaxRounds = 10)

$ErrorActionPreference = 'Stop'
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$root    = $PSScriptRoot
$tracker = Join-Path $root 'NANOARCH_TRACKER.md'
$logDir  = Join-Path $root 'target\tracker-logs'

# Items that cannot be built honestly without a user decision, an external library/service,
# or a statutory source. They are marked [?] and never attempted by this script.
$needsDecision = @(1, 3, 9, 12, 32, 53, 71, 78, 79, 82, 83, 94, 95, 96)

function Stop-Run([string]$Message) {
    Write-Host "STOP: $Message" -ForegroundColor Red
    exit 1
}

function Invoke-Checks {
    # Real verification, run by this script (not by the model). Returns $true only if all pass.
    $steps = @(
        @{ Dir = (Join-Path $root 'nnx-core'); Args = @('test') },
        @{ Dir = (Join-Path $root 'nnx-core'); Args = @('clippy', '--all-targets', '--', '-D', 'warnings') },
        @{ Dir = $root;                        Args = @('test') },
        @{ Dir = $root;                        Args = @('clippy', '--all-targets', '--', '-D', 'warnings') }
    )
    foreach ($s in $steps) {
        Push-Location $s.Dir
        try {
            Write-Host ("cargo " + ($s.Args -join ' ') + "   [" + $s.Dir + "]") -ForegroundColor DarkGray
            & cargo @($s.Args)
            if ($LASTEXITCODE -ne 0) { return $false }
        } finally { Pop-Location }
    }
    return $true
}

function Get-ItemState([int]$Number) {
    $line = Select-String -Path $tracker -Pattern ("^- \[(.)\] {0}\." -f $Number) | Select-Object -First 1
    if ($null -eq $line) { return $null }
    return $line.Matches[0].Groups[1].Value
}

Set-Location $root
if (-not (Test-Path $tracker)) { Stop-Run 'NANOARCH_TRACKER.md not found.' }
if (-not (Get-Command claude -ErrorAction SilentlyContinue)) { Stop-Run 'claude CLI not found on PATH.' }
if (-not (Get-Command cargo  -ErrorAction SilentlyContinue)) { Stop-Run 'cargo not found on PATH.' }

# 1. Working tree must be clean apart from the tracker and this script.
$dirty = git status --porcelain | Where-Object { $_ -notmatch 'NANOARCH_TRACKER\.md$' -and $_ -notmatch 'run_tracker\.ps1$' }
if ($dirty) { Stop-Run ("Working tree has other uncommitted changes:`n" + ($dirty -join "`n")) }

# 2. Never work on main.
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -eq 'main' -or $branch -eq 'master') {
    $branch = 'tracker-run-' + (Get-Date -Format 'yyyyMMdd-HHmm')
    git switch -c $branch
    if ($LASTEXITCODE -ne 0) { Stop-Run 'Could not create work branch.' }
    Write-Host "Working on new branch $branch" -ForegroundColor Cyan
}

# 3. Mark [?] items up front (idempotent) and add a legend once.
$text = Get-Content -Path $tracker -Raw -Encoding UTF8
foreach ($n in $needsDecision) {
    $text = $text -replace ("(?m)^- \[ \] {0}\." -f $n), ("- [?] {0}." -f $n)
}
if ($text -notmatch 'Legend:') {
    $text = $text -replace '(?m)^(# .+)$', ('$1' + "`r`n`r`n> Legend: [x] done and verified, [?] needs a user decision or an external source (skipped by the runner), [ ] pending.")
}
Set-Content -Path $tracker -Value $text -Encoding UTF8

# 4. Baseline: the project must already be green, otherwise a later failure could not be attributed.
Write-Host 'Baseline checks...' -ForegroundColor Cyan
if (-not (Invoke-Checks)) { Stop-Run 'Baseline is not green. Fix it first; nothing was changed by claude.' }

git add NANOARCH_TRACKER.md run_tracker.ps1
git commit -m "chore: add execution tracker and bounded runner" | Out-Null
New-Item -ItemType Directory -Force $logDir | Out-Null

$allowed = @(
    'Bash(cargo test:*)', 'Bash(cargo clippy:*)', 'Bash(cargo build:*)', 'Bash(cargo check:*)', 'Bash(cargo fmt:*)',
    'Bash(git status:*)', 'Bash(git diff:*)', 'Bash(git log:*)', 'Bash(git add:*)', 'Bash(git commit:*)'
)
$denied = @(
    'Bash(git add -A:*)', 'Bash(git add .:*)', 'Bash(git add --all:*)', 'Bash(git push:*)', 'Bash(git reset:*)',
    'Bash(git clean:*)', 'Bash(git checkout:*)', 'Bash(git switch:*)', 'Bash(git rebase:*)', 'Bash(git merge:*)'
)

for ($round = 1; $round -le $MaxRounds; $round++) {
    $pending = Select-String -Path $tracker -Pattern '^- \[ \] (\d+)\.' | Select-Object -First 1
    if ($null -eq $pending) {
        Write-Host 'No pending items left ([ ]). Done.' -ForegroundColor Green
        exit 0
    }
    $n = [int]$pending.Matches[0].Groups[1].Value
    $headBefore = (git rev-parse HEAD).Trim()
    Write-Host ("=== Round {0}/{1}: tracker item {2} ===" -f $round, $MaxRounds, $n) -ForegroundColor Cyan

    $prompt = @"
You are the lead engineer of NanoArch (Rust, see CLAUDE.md in this repo; follow it exactly).
Work on tracker item number $n in NANOARCH_TRACKER.md and on nothing else. Read the tracker line and the project first.

Rules:
- If the item already exists partly, build on it and complete it. No TODO, no stubs, no placeholder behaviour.
- Engineering logic belongs in nnx-core. The page and backend.rs only send inputs and draw results.
- Never invent a code value, load, soil, price or setback. A missing input is reported by name and the calculation does not run. Every statutory value needs its source (code, edition, clause, page) from the reference PDFs; otherwise it stays unset.
- Add tests for the new behaviour.
- If the item cannot be finished honestly because it needs a user decision, an external library or service, or a statutory text you cannot read, do NOT fake it: change its marker to [?], add one short line saying exactly what is needed, commit the tracker, and stop.
- Verify with: cargo test --manifest-path nnx-core/Cargo.toml ; cargo clippy --manifest-path nnx-core/Cargo.toml --all-targets -- -D warnings ; cargo test ; cargo clippy --all-targets -- -D warnings. Fix root causes; never add ignore attributes or skip tests.
- When all four pass: set the item to [x] in NANOARCH_TRACKER.md and append a short note (files changed, test result lines). Then stage by file name only (never git add -A or git add .) and make exactly one Conventional Commit that includes the tracker.
- Do not push. Do not touch anything outside C:\Projects\NanoArch.
"@

    $log = Join-Path $logDir ("round{0:D2}-item{1}.log" -f $round, $n)
    $ErrorActionPreference = 'Continue'; $ErrorActionPreference = 'Continue'; $prompt | & claude -p --permission-mode acceptEdits --allowedTools @allowed --disallowedTools @denied 2>&1 | Tee-Object -FilePath $log; $ErrorActionPreference = 'Stop'; $ErrorActionPreference = 'Stop'
    $claudeExit = $LASTEXITCODE
    if ($claudeExit -ne 0) { Stop-Run "claude exited with code $claudeExit (log: $log)" }

    # Independent verification.
    $state = Get-ItemState $n
    if ($state -eq ' ') { Stop-Run "Item $n is still [ ] after the round: no progress (log: $log)" }
    if ($null -eq $state) { Stop-Run "Item $n line was not found in the tracker after the round." }

    $left = git status --porcelain
    if ($left) { Stop-Run ("Working tree is dirty after round ${round}:`n" + ($left -join "`n")) }

    $headAfter = (git rev-parse HEAD).Trim()
    if ($headAfter -eq $headBefore) { Stop-Run "No commit was made in round $round." }

    if (-not (Invoke-Checks)) { Stop-Run "Checks failed after round $round (item $n, state [$state]). Inspect the last commit and $log" }

    Write-Host ("Round {0} OK: item {1} -> [{2}], commit {3}" -f $round, $n, $state, $headAfter.Substring(0, 7)) -ForegroundColor Green
}

Write-Host "Reached MaxRounds ($MaxRounds). Review the branch before merging." -ForegroundColor Yellow
exit 0
