# Compares how the original and tflip name a connection's path, on the
# Windows VM (see LOCAL.md), with Y: mapped to \\host.lan\Data (this machine's
# ~/src/ace-sdk) and mapped.exe and a table in its mapped\ folder:
#
#     net use Y: \\host.lan\Data & powershell -ExecutionPolicy Bypass -File paths.ps1
#
# The original's DLL is C:\acesdk\ace32.dll; tflip's, C:\work\tflip_ace32.dll.
New-Item -ItemType Directory -Force C:\work\rel | Out-Null
Set-Location C:\work
foreach ($dll in 'C:\acesdk\ace32.dll', 'C:\work\tflip_ace32.dll') {
  "===== $dll"
  foreach ($p in 'rel', '.\rel', 'C:rel', 'C:\work\rel\', 'C:\', 'C:', 'C:\nope', 'C:\work\rel\..\.\rel', 'C:\WORK\Rel',
                 'Y:\mapped', 'Y:\', 'Y:', 'Y:\nope\deeper', 'y:/mapped/sub/', '\\host.lan\Data\mapped\', '\\127.0.0.1\C$\work') {
    & Y:\mapped\mapped.exe $dll $p | Select-String 'types=1' -Context 0,3
  }
}
