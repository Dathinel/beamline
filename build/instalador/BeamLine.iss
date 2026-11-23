; Instalador de BeamLine (Inno Setup 7). Lo compila build\distribuir.ps1.
; Por usuario y sin permisos de administrador: {userpf}\BeamLine (%LOCALAPPDATA%\Programs\BeamLine).

#ifndef Version
  #error Falta /DVersion
#endif
#ifndef Origen
  #error Falta /DOrigen
#endif
#ifndef Licencia
  #error Falta /DLicencia
#endif

[Setup]
AppId={{4E2B6C1D-7A3F-4C58-9B0E-2D61F8A5C3B7}}
AppName=BeamLine
AppVersion={#Version}
AppVerName=BeamLine {#Version}
AppPublisher=Dathinel
AppPublisherURL=https://github.com/Dathinel/beamline
AppSupportURL=https://github.com/Dathinel/beamline/issues
AppUpdatesURL=https://github.com/Dathinel/beamline/releases
AppCopyright=Copyright (C) 2026 Dathinel
VersionInfoVersion={#Version}.0
VersionInfoProductName=BeamLine
VersionInfoDescription=Instalador de BeamLine
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=
DefaultDirName={userpf}\BeamLine
DisableProgramGroupPage=yes
DisableDirPage=auto
UsePreviousAppDir=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
LicenseFile={#Licencia}
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=BeamLine
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes
OutputBaseFilename=BeamLine-{#Version}-instalador

[Languages]
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "escritorio"; Description: "Crear un acceso directo en el escritorio"; Flags: unchecked

[Files]
Source: "{#Origen}\*"; DestDir: "{app}"; Excludes: "*.pdb"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\BeamLine"; Filename: "{app}\BeamLine.exe"; Comment: "Enviar archivos entre equipos"
Name: "{autodesktop}\BeamLine"; Filename: "{app}\BeamLine.exe"; Comment: "Enviar archivos entre equipos"; Tasks: escritorio

[Run]
Filename: "{app}\BeamLine.exe"; Description: "Abrir BeamLine"; Flags: nowait postinstall skipifsilent
