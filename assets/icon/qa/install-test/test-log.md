# Pod installer test 2026-11-09 15:02:38

- installer: C:\Users\User\dev\workspaces\ide\fu-icon\dist\Pod-Setup-0.1.0-x64.exe (12.2 MB, sha256 6c7178c64f7d7cb331e8c90fa91d6a055f4a8b2639f8fda55cce179c596b977f)
- prefix: C:\Users\User\AppData\Local\Temp\pod-install-test (app: C:\Users\User\AppData\Local\Temp\pod-install-test\app), pipe: podd-installer-test
- before: start menu Pod.lnk=False, desktop Pod.lnk=False, uninstall key=False

## 1. Fresh install (/VERYSILENT /TASKS=addtopath)
- PASS setup exit code 0 - exit 0
- PASS file pod-app.exe
- PASS file podd.exe
- PASS file pod-holder.exe
- PASS file pod.exe
- PASS file pod-hook.exe
- PASS file conpty.dll
- PASS file x64\OpenConsole.exe
- PASS file LICENSE.txt
- PASS file THIRD-PARTY-NOTICES.txt
- PASS file README.txt
- PASS file licenses\ConPTY-LICENSE.txt
- PASS file unins000.exe
- PASS Start menu shortcut - C:\Users\User\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Pod.lnk
- PASS shortcut icon - C:\Users\User\AppData\Local\Temp\pod-install-test\app\pod-app.exe,0
- PASS shortcut target - C:\Users\User\AppData\Local\Temp\pod-install-test\app\pod-app.exe
- PASS no desktop shortcut (task unchecked)
- PASS HKCU uninstall entry - Pod 0.1.0, InstallLocation=C:\Users\User\AppData\Local\Temp\pod-install-test\app\
- PASS PATH has app dir
- PASS hooks checked, not installed silently
- PASS hooks dir untouched (prefix)
- PASS pod-app version resource - 0.1.0, Pod agent coding IDE (Apache-2.0)
- PASS podd version resource - 0.1.0, Pod terminal daemon (Apache-2.0)
- PASS pod-holder version resource - 0.1.0, Pod terminal host (Apache-2.0)
- PASS pod version resource - 0.1.0, Pod command line (Apache-2.0)
- PASS pod-hook version resource - 0.1.0, Pod agent hook relay (Apache-2.0)
- PASS uninstall icon - C:\Users\User\AppData\Local\Temp\pod-install-test\app\pod-app.exe

## 2. Installed binaries
- PASS pod --version - pod 0.1.0
- PASS podd --version - podd 0.1.0
    pod hooks status: pod-hook: C:\Users\User\AppData\Local\Temp\pod-install-test\localappdata\Pod\bin\pod-hook.exe (missing) | claude: C:\Users\User\.claude\settings.json - 13 Pod event(s), 16 other hook group(s) |   runs C:/Users/User/AppData/Local/Pod/bin/pod-hook.exe | codex: C:\Users\User\.codex\hooks.json - 9 Pod event(s), 9 other hook group(s) |   runs C:/Users/User/AppData/Local/Pod/bin/pod-hook.exe
- PASS pod hooks status runs
- PASS pod-app --ui-fixture opens a window
- PASS runtime large window icon
- PASS runtime small window icon
- PASS runtime class icon
- PASS app window on the secondary monitor - rect 2549,224-4491,1446
    screenshot: app-installed-fixture.png
- PASS pod run (installed podd + holder) - t1
- PASS holder and podd run from the install dir - holder pid 46268, podd pid 65484
    terminal conpty: bundled:C:\Users\User\AppData\Local\Temp\pod-install-test\app\conpty.dll
- PASS terminal uses the bundled ConPTY of the install - bundled:C:\Users\User\AppData\Local\Temp\pod-install-test\app\conpty.dll

## 3. Upgrade over itself while podd + 1 terminal run
- PASS upgrade exit code 0 - exit 0
- PASS podd stopped gracefully (pod server stop)
- PASS holder survived (same pid) - pid 46268
- PASS in-use files moved aside - conpty.dll.old-20261109150521, pod-holder.exe.old-20261109150521, OpenConsole.exe.old-20261109150521
- PASS new pod-holder.exe in place
- PASS terminal still listed as running after upgrade (new podd re-attached) - ID  NAME  STATE    STATUS   PID    SIZE    COMMAND                         CWD | t1  keep  running  unknown  36476  120x30  cmd.exe /k echo installer-test  C:\Users\User\dev\workspaces\ide\fu-icon
- PASS terminal output intact
- PASS new podd started from the install dir - pid 61060

## 4. Silent uninstall with a running terminal (no /STOPTERMINALS)
- PASS uninstall refused (non-zero exit) - exit 1
- PASS install intact
- PASS terminal still running

## 5. Silent uninstall /STOPTERMINALS (keep user data)
- PASS uninstall exit code 0 - exit 0
- PASS no Pod processes left
- PASS app dir removed
- PASS Start menu shortcut removed
- PASS uninstall entry removed
- PASS PATH restored
- PASS user data kept (state dir)

## 6. Reinstall without PATH task, then uninstall /PURGEDATA
- PASS reinstall exit code 0 - exit 0
- PASS PATH unchanged when task is off - len 636 vs 636; has dir: False
- PASS uninstall exit code 0 - exit 0
- PASS app dir removed
- PASS state + log + app config removed
- PASS Start menu, uninstall entry, PATH back to before
- PASS desktop unchanged

Result: 63 passed, 0 failed
