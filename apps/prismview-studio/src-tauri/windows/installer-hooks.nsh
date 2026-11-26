; Ganchos do instalador NSIS (tauri.installer.conf.json).
; O motor (prismview-engine.exe) roda sozinho, sem o Studio: o NSIS do Tauri só fecha o Studio,
; então o motor é fechado aqui antes de trocar/apagar o .exe (Windows trava arquivo em uso).

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /T /IM prismview-engine.exe'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /T /IM prismview-engine.exe'
  Pop $0
!macroend

; Desinstalação de verdade (não atualização): tira o motor e o Studio do "iniciar com o Windows".
; Os projetos do usuário em %APPDATA%\PrismView ficam.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PrismViewEngine"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PrismView"
  ${EndIf}
!macroend
