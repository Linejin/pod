# Synthetic trees test the activation predicate, not the native app.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
. (Join-Path $root 'tools/visual/uia-required.ps1')
$full = (@('ControlType.TabItem 설정', 'ControlType.Tab 설정 분류', 'ControlType.ComboBox 스타일', 'ControlType.Button 상태') + (1..20 | ForEach-Object { "ControlType.Text fixture $_" })) -join "`n"
if ((Test-PodDockActivationTree $full).Count) { throw 'Valid settings tree rejected' }
if (-not (Test-PodDockActivationTree ($full.Replace('설정 분류', 'missing'))).Count) { throw 'Missing dock content accepted' }
if (-not (Test-PodDockActivationTree ($full + "`nControlType.TabItem 상태")).Count) { throw 'Unexpected inspector accepted' }
'PASS: activation tree accepts settings/status button, rejects missing dock and unexpected inspector'
