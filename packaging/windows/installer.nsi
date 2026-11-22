; Windows installer for Try MAN. Built by build-windows.sh:
;   makensis -DVERSION=10 -DSRCDIR=<folder> -DOUTFILE=<setup.exe> installer.nsi
; Installs for the current user only, so no administrator password is needed.

Unicode true
!include "MUI2.nsh"

!define APPNAME "Try MAN"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\TryMAN"

Name "${APPNAME}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Try MAN"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "${APPNAME} ${VERSION}"

!define MUI_ICON "${SRCDIR}\try-man.ico"
!define MUI_UNICON "${SRCDIR}\try-man.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\bin\try-man.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Try MAN"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
    SetOutPath "$INSTDIR"
    File /r "${SRCDIR}\*"
    WriteUninstaller "$INSTDIR\Uninstall.exe"

    ; Shortcuts start in the bin folder, next to the DLLs.
    SetOutPath "$INSTDIR\bin"
    CreateShortcut "$SMPROGRAMS\Try MAN.lnk" "$INSTDIR\bin\try-man.exe" "" "$INSTDIR\try-man.ico"
    CreateShortcut "$DESKTOP\Try MAN.lnk" "$INSTDIR\bin\try-man.exe" "" "$INSTDIR\try-man.ico"

    WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
    WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "MAN Corporation"
    WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\try-man.ico"
    WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
    Delete "$SMPROGRAMS\Try MAN.lnk"
    Delete "$DESKTOP\Try MAN.lnk"
    RMDir /r "$INSTDIR"
    DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd
