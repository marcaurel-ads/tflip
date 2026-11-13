# Environment check for the AdsFindServers entry in docs/ADS_BUGS.md.
#
# AdsFindServers returns 50 (not an ADS error code) on the Windows VM. This test
# shows that is NOT because the environment cannot carry multicast/broadcast:
# it joins a multicast group, sends to it, and receives the loopback, then does
# the same for a 255.255.255.255 broadcast. Both succeed on the VM (Red Hat
# VirtIO NIC on a TAP/bridge), while AdsFindServers on the same box still
# returns 50 -- so the refusal is internal to the engine's discovery path, not
# a missing network capability.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File mcast-selftest.ps1
$ErrorActionPreference = 'Continue'

try {
  $grp = [Net.IPAddress]::Parse('239.7.7.7'); $port = 50877
  $rx = New-Object Net.Sockets.UdpClient
  $rx.Client.SetSocketOption('Socket', 'ReuseAddress', $true)
  $rx.Client.Bind((New-Object Net.IPEndPoint([Net.IPAddress]::Any, $port)))
  $rx.JoinMulticastGroup($grp)
  $rx.Client.ReceiveTimeout = 2000
  $tx = New-Object Net.Sockets.UdpClient; $tx.MulticastLoopback = $true
  $b = [Text.Encoding]::ASCII.GetBytes('ping')
  [void]$tx.Send($b, $b.Length, (New-Object Net.IPEndPoint($grp, $port)))
  $ep = New-Object Net.IPEndPoint([Net.IPAddress]::Any, 0); $d = $rx.Receive([ref]$ep)
  Write-Output ("MULTICAST ok: '" + [Text.Encoding]::ASCII.GetString($d) + "' from " + $ep.Address)
} catch { Write-Output ("MULTICAST fail: " + $_.Exception.GetType().Name + ": " + $_.Exception.Message) }
finally { if ($rx) { $rx.Close() }; if ($tx) { $tx.Close() } }
$rx = $null; $tx = $null

try {
  $port = 50878
  $rx = New-Object Net.Sockets.UdpClient
  $rx.Client.SetSocketOption('Socket', 'ReuseAddress', $true)
  $rx.Client.Bind((New-Object Net.IPEndPoint([Net.IPAddress]::Any, $port)))
  $rx.Client.ReceiveTimeout = 2000
  $tx = New-Object Net.Sockets.UdpClient; $tx.EnableBroadcast = $true
  $b = [Text.Encoding]::ASCII.GetBytes('bcast')
  [void]$tx.Send($b, $b.Length, (New-Object Net.IPEndPoint([Net.IPAddress]::Broadcast, $port)))
  $ep = New-Object Net.IPEndPoint([Net.IPAddress]::Any, 0); $d = $rx.Receive([ref]$ep)
  Write-Output ("BROADCAST ok: '" + [Text.Encoding]::ASCII.GetString($d) + "' from " + $ep.Address)
} catch { Write-Output ("BROADCAST fail: " + $_.Exception.GetType().Name + ": " + $_.Exception.Message) }
finally { if ($rx) { $rx.Close() }; if ($tx) { $tx.Close() } }
