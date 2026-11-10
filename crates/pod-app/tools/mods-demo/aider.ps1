# Fixture only: prints Aider-like screens so the aider-status example mod has
# something to read (docs/16 §10). It is not Aider and talks to no model.
$ErrorActionPreference = "Stop"
$esc = [char]27
Write-Host "Aider v0.0.0-fixture"
Write-Host "Main model: fixture-model with diff edit format"
Write-Host "Git repo: .git with 12 files"
Write-Host "Repo-map: using 1024 tokens, auto refresh"
Write-Host ""
Write-Host -NoNewline "> "
Start-Sleep -Seconds 4
Write-Host "add input validation to parse_args"
Write-Host ""
Write-Host "Waiting for fixture-model"
foreach ($i in 0..10) { Write-Host -NoNewline "`r$([char]0x280B) Thinking $i s"; Start-Sleep -Milliseconds 400 }
Write-Host ""
Write-Host "src/cli.rs"
Write-Host "<<<<<<< SEARCH (fixture diff)"
Write-Host ">>>>>>> REPLACE"
Write-Host ""
Write-Host -NoNewline "Run cargo test? (Y)es/(N)o [Yes]: "
Start-Sleep -Seconds 600
