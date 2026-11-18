; My Librarian - NSIS hooks (E10-01).
; Du lieu app nam o <thu muc cai>\data (ban cu: %APPDATA%\my-librarian). Tauri khong tu xoa hai thu muc nay,
; nen khi go cai dat va ban tick "Delete the application data" thi xoa them. Cai de (cap nhat) khong dong toi.
; File sach trong thu vien KHONG bao gio bi dong toi.

!macro NSIS_HOOK_POSTUNINSTALL
  ; Luat tuong lua cong 47470 do app them (nut "Sua" o man iPad, I3-07). Cai dat che do currentUser
  ; khong co quyen Admin nen chi xoa khi luat ton tai, kem hop UAC. Cap nhat thi giu luat.
  ${If} $UpdateMode <> 1
    nsExec::Exec 'netsh advfirewall firewall show rule name="My Librarian sync (TCP 47470)"'
    Pop $0
    ${If} $0 = 0
      ExecShell "runas" "netsh" 'advfirewall firewall delete rule name="My Librarian sync (TCP 47470)"' SW_HIDE
    ${EndIf}
  ${EndIf}

  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$APPDATA\my-librarian"
    RmDir /r "$INSTDIR\data"
    RmDir "$INSTDIR"
  ${EndIf}
!macroend
