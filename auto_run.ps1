$ErrorActionPreference = "Continue"
$tracker = "NANOARCH_TRACKER.md"
New-Item -ItemType Directory -Force -Path "target\tracker-logs" | Out-Null

$round = 0
$stuckCount = 0
$lastHash = ""

while (Select-String -Path $tracker -Pattern "- \[ \]" -Quiet) {
    $round++
    $currentHash = (Get-FileHash $tracker -Algorithm MD5).Hash

    if ($currentHash -eq $lastHash) {
        $stuckCount++
        if ($stuckCount -ge 2) {
            $utf8 = New-Object System.Text.UTF8Encoding($false)
            $text = [System.IO.File]::ReadAllText((Resolve-Path $tracker).Path, $utf8)
            $text = [regex]::Replace($text, '- \[ \]', '- [!]', 1)
            [System.IO.File]::WriteAllText((Resolve-Path $tracker).Path, $text, $utf8)
            Write-Host ">>> Skipped stuck item as [!] and moving to next..." -ForegroundColor Yellow
            $stuckCount = 0
            continue
        }
    } else {
        $stuckCount = 0
    }
    $lastHash = $currentHash

    Write-Host "`n================ Round $round ================" -ForegroundColor Cyan
    $logFile = "target\tracker-logs\round_$round.log"

    $prompt = "Read NANOARCH_TRACKER.md and pick the first unchecked item [ ]. Implement it fully with production quality, run cargo test to verify, and fix any errors. If it requires an external paid library or human decision, mark it [?] with the reason. Once verified, mark it [x] in NANOARCH_TRACKER.md and finish."

    $prompt | & claude -p --dangerously-skip-permissions 2>&1 | Tee-Object -FilePath $logFile

    git add -A 2>$null
    git commit -m "auto: round $round" 2>$null

    Start-Sleep -Seconds 3
}

Write-Host "`n>>> All items in NANOARCH_TRACKER.md are done!" -ForegroundColor Green
