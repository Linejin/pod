# Pretends to be an agent by printing OSC 7501 status reports (docs/04, docs/07 §4.1),
# so the status UI can be exercised without prompting a real agent.
# Run inside a pod terminal: pod run --name sim -- powershell -File agent-sim.ps1
param([int]$Step = 4)
$e = [char]27; $bel = [char]7
function Report([string]$payload) { [Console]::Out.Write("$e]7501;$payload$bel"); [Console]::Out.Flush() }
Report "state=idle:app=agent-sim"; "agent-sim: idle"
Start-Sleep $Step
Report "state=working:app=agent-sim"; "agent-sim: working on the task..."
Start-Sleep $Step
Report "state=blocked:kind=permission:app=agent-sim"; "agent-sim: allow running 'cargo test'? (blocked)"
Start-Sleep ($Step * 2)
Report "state=working:app=agent-sim"; "agent-sim: permission granted, working..."
Start-Sleep $Step
Report "state=done:app=agent-sim"; "agent-sim: done."
while ($true) { Start-Sleep 60 }
