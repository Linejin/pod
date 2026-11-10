# SPDX-License-Identifier: Apache-2.0
param([string]$State = 'blocked', [string]$Kind = 'permission')
# Fixture for the fix2 B5 check: emit an OSC 7501 status report like an agent would.
$payload = "state=$State" + $(if ($State -eq 'blocked') { ":kind=$Kind" } else { '' }) + ':app=fixture'
[Console]::Write([string][char]27 + ']7501;' + $payload + [string][char]7)
