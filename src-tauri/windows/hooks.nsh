; Fonths NSIS hooks (perMachine elevated installer).
; Cleanly stop any running headless `--daemon` first via `--quit-daemon`, then ask any UI
; instance to close cleanly (WM_CLOSE) and wait for session unload. Never touch Tip builds (\Tip\).
; Documents\Fonths and cache data are always preserved.

!macro FM_STOP_DAEMON
  IfFileExists "$INSTDIR\fonths.exe" 0 +3
    nsExec::ExecToLog '"$INSTDIR\fonths.exe" --quit-daemon'
    Pop $R9
!macroend

!macro FM_ASK_CLOSE exeName
  nsExec::ExecToLog 'powershell -NoProfile -WindowStyle Hidden -Command "$procs = Get-Process -Name ''${exeName}'' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ($_.Path -notmatch ''\\[Tt]ip\\'') }; foreach ($p in $procs) { $p.CloseMainWindow() | Out-Null }; for ($i=0; $i -lt 20; $i++) { $alive = Get-Process -Name ''${exeName}'' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ($_.Path -notmatch ''\\[Tt]ip\\'') }; if (-not $alive) { break }; Start-Sleep -Milliseconds 250 }; $rem = Get-Process -Name ''${exeName}'' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ($_.Path -notmatch ''\\[Tt]ip\\'') -and ($_.MainWindowHandle -eq 0) }; foreach ($p in $rem) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }"'
  Pop $R9
!macroend

; Clean up any legacy per-user install under %LOCALAPPDATA%\Font Manager (exe + uninstall.exe
; + HKCU uninstall key) while strictly keeping gdi-maps, scan-index.json, and user data intact.
!macro FM_CLEAN_LEGACY_PER_USER
  nsExec::ExecToLog 'powershell -NoProfile -WindowStyle Hidden -Command "$dir = Join-Path $env:LOCALAPPDATA ''Font Manager''; if (Test-Path $dir) { Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $dir ''font-manager.exe''), (Join-Path $dir ''fonths.exe''), (Join-Path $dir ''uninstall.exe'') }; Remove-Item -Recurse -Force -ErrorAction SilentlyContinue ''HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Font Manager''"'
  Pop $R9
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro FM_STOP_DAEMON
  !insertmacro FM_ASK_CLOSE "fonths"
  !insertmacro FM_ASK_CLOSE "font-manager"
  !insertmacro FM_ASK_CLOSE "${MAINBINARYNAME}"
  !insertmacro FM_CLEAN_LEGACY_PER_USER
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro FM_STOP_DAEMON
  !insertmacro FM_ASK_CLOSE "fonths"
  !insertmacro FM_ASK_CLOSE "font-manager"
  !insertmacro FM_ASK_CLOSE "${MAINBINARYNAME}"
  IfFileExists "$INSTDIR\fonths.exe" 0 +3
    nsExec::ExecToStack '"$INSTDIR\fonths.exe" --deactivate-all-and-exit'
    Pop $R9
!macroend
