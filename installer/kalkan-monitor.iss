; Instalador do Kalkan Monitor (Windows) — Inno Setup 6.
;
; Gerado por `installer/build.ps1`, que compila o app, publica o sidecar e baixa
; o setup oficial do PawnIO (versão fixa, SHA-256 conferido). Não rode o ISCC
; direto sem esses artefatos.
;
; Instala:
;   * kalkan-monitor.exe  — o dashboard (Rust/Slint), que fala com o cooler;
;   * kalkan-sensord.exe  — sidecar de sensores (LibreHardwareMonitor);
;   * PawnIO              — driver de kernel compatível com HVCI que o
;                           LibreHardwareMonitor usa. Instalado em modo silencioso
;                           só se ausente ou < 2.0 (mesma regra do LHM). O
;                           desinstalador NÃO remove o PawnIO: outros programas
;                           (ex.: LibreHardwareMonitor) usam o mesmo driver.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef PawnIOSetup
  #define PawnIOSetup "..\target\installer\PawnIO_setup.exe"
#endif

[Setup]
AppId={{6F2A9C4E-3B1D-4E8A-9C57-1D0B8E4F2A63}
AppName=Kalkan Monitor
AppVersion={#AppVersion}
AppPublisher=Kalkan Monitor RS contributors
AppPublisherURL=https://github.com/medeiroz/kalkan-monitor-rs
DefaultDirName={autopf}\Kalkan Monitor
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=KalkanMonitor-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\kalkan-monitor.exe
CloseApplications=yes

[Languages]
Name: "ptbr"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\kalkan-monitor.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\sidecar\kalkan-sensord\bin\Release\net8.0\win-x64\publish\kalkan-sensord.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PawnIOSetup}"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: PawnIONeeded

[Icons]
Name: "{autoprograms}\Kalkan Monitor"; Filename: "{app}\kalkan-monitor.exe"
Name: "{autodesktop}\Kalkan Monitor"; Filename: "{app}\kalkan-monitor.exe"; Tasks: desktopicon

[Run]
; PawnIO antigo (< 2.0): o setup recusa instalar por cima, então desinstala antes.
Filename: "{code:PawnIOUninstaller}"; Parameters: "-uninstall -silent"; StatusMsg: "Atualizando o driver PawnIO..."; Flags: runhidden waituntilterminated; Check: PawnIOOutdated
Filename: "{tmp}\PawnIO_setup.exe"; Parameters: "-install -silent"; StatusMsg: "Instalando o driver PawnIO..."; Flags: runhidden waituntilterminated; Check: PawnIONeeded
Filename: "{app}\kalkan-monitor.exe"; Description: "{cm:LaunchProgram,Kalkan Monitor}"; Flags: nowait postinstall skipifsilent

[Code]
const
  PawnIOKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO';

var
  PawnIOChecked, PawnIOMissing, PawnIOOld: Boolean;

{ Lê o estado do PawnIO uma vez só: depois da desinstalação do PawnIO antigo o
  registro muda, e os Checks seguintes precisam ver o estado original. }
procedure CheckPawnIO;
var
  Ver: String;
begin
  if PawnIOChecked then
    Exit;
  PawnIOChecked := True;
  if not RegQueryStringValue(HKLM64, PawnIOKey, 'DisplayVersion', Ver) then
  begin
    PawnIOMissing := True;
    Exit;
  end;
  PawnIOOld := StrToIntDef(Copy(Ver, 1, Pos('.', Ver + '.') - 1), 0) < 2;
end;

function PawnIOOutdated: Boolean;
begin
  CheckPawnIO;
  Result := PawnIOOld;
end;

function PawnIONeeded: Boolean;
begin
  CheckPawnIO;
  Result := PawnIOMissing or PawnIOOld;
end;

function PawnIOUninstaller(Param: String): String;
begin
  if not RegQueryStringValue(HKLM64, PawnIOKey, 'InstallLocation', Result) then
    Result := ExpandConstant('{commonpf64}\PawnIO');
  Result := AddBackslash(Result) + 'uninstall.exe';
end;
