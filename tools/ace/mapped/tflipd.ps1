# End to end on the Windows VM: X: mapped to its own share tdata (C:\tdata), served
# by tflipd.exe, with Y: as in paths.ps1 holding mapped.exe, tflipd.exe and
# tflip\ace32.dll; the original has no server there and fails 6420.
New-Item -ItemType Directory -Force C:\tdata\sub | Out-Null
Copy-Item -Force Y:\mapped\all_types.ad? C:\tdata\sub\
net share tdata /delete /y 2>$null | Out-Null
net share tdata=C:\tdata /grant:everyone,full | Out-Null
net use X: /delete /y 2>$null | Out-Null
net use X: \\127.0.0.1\tdata | Out-Null
Copy-Item -Force Y:\mapped\tflip\ace32.dll C:\work\tflip_ace32.dll
Copy-Item -Force Y:\mapped\tflipd.exe C:\work\tflipd.exe
Get-Process tflipd -ErrorAction SilentlyContinue | Stop-Process -Force
$d = Start-Process -FilePath C:\work\tflipd.exe -ArgumentList '--share','tdata=C:\tdata','--listen','127.0.0.1:6262','--state','C:\work\tstate' -RedirectStandardOutput C:\work\tflipd.out -RedirectStandardError C:\work\tflipd.err -PassThru -NoNewWindow
Start-Sleep 3
$fp = (Select-String -Path C:\work\tflipd.out -Pattern 'fingerprint: (\S+)').Matches[0].Groups[1].Value
"fingerprint $fp"
$env:TFLIP_SERVER_FINGERPRINT = $fp
$env:TFLIPD_DEBUG = 'requests'
Set-Location C:\work
foreach ($dll in 'C:\acesdk\ace32.dll', 'C:\work\tflip_ace32.dll') {
  "===== $dll"
  & Y:\mapped\mapped.exe $dll 'X:\sub' 'X:\sub\all_types.adt'
  & Y:\mapped\mapped.exe $dll 'X:\sub' 'all_types.adt'
}
Stop-Process $d -Force
"--- tflipd stdout"; Get-Content C:\work\tflipd.out | Select-Object -First 15
"--- tflipd stderr"; Get-Content C:\work\tflipd.err | Select-Object -First 30
