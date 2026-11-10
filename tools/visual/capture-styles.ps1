# Captures one scene (default: workspace) in every built-in style theme, dark and light
# (docs/10 §D5, THEMES stage 1). Fixture only, secondary monitor, isolated config per run.
#
#   powershell -NoProfile -File tools/visual/capture-styles.ps1 [-Scenes workspace] [-Output crates/pod-app/qa/themes]
#
# Files: <output>/<dark|light>-<scene>-<style>.png (+ UIA tree). Compare them with
# docs/design/gallery/images/<NN>-*-<dark|light>.png; notes live in crates/pod-app/qa/themes/README.md.
param(
 [string[]]$Scenes = @('workspace'),
 [string]$Output = 'crates/pod-app/qa/themes',
 [string[]]$Styles = @()
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $Styles.Count) {
 $Styles = Get-ChildItem (Join-Path $root 'crates/pod-ui/themes') -Filter '*.json' | ForEach-Object { $_.BaseName }
}
$all = @()
foreach ($style in $Styles) {
 & (Join-Path $PSScriptRoot 'capture.ps1') -Output $Output -Scenes $Scenes -Style $style
 $all += Get-Content -Raw -Encoding UTF8 (Join-Path $Output 'manifest.json') | ConvertFrom-Json
}
$all | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $Output 'manifest.json')
