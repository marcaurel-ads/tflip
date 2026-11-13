# Decisive test for the AdsFindServers entry in docs/ADS_BUGS.md.
#
# AdsFindServers IP-multicasts a 50-byte discovery request to 224.0.1.55:2989
# (seen with Process Monitor) and returns 50 (not an ADS error code) when nothing
# answers. This listener joins that group and echoes each request straight back
# to the sender. With it running, every valid ulOptions flips from 50 to rc=0
# and returns the 13-column result cursor (empty rows only because an echo is not
# a valid server-info record) -- proving the 50 is "no reply to the discovery
# multicast", and that per the SDK docs an empty result should be a table, not
# a NULL handle.
#
#   # start this first, then run findservers.exe within ~12s:
#   powershell -NoProfile -ExecutionPolicy Bypass -File responder.ps1
$grp = [Net.IPAddress]::Parse('224.0.1.55'); $port = 2989
$log = Join-Path $PSScriptRoot 'responder.log'
Remove-Item $log -ErrorAction SilentlyContinue
try {
  $udp = New-Object Net.Sockets.UdpClient
  $udp.Client.SetSocketOption('Socket', 'ReuseAddress', $true)
  $udp.Client.Bind((New-Object Net.IPEndPoint([Net.IPAddress]::Any, $port)))
  $udp.JoinMulticastGroup($grp)
  $udp.Client.ReceiveTimeout = 12000
  Add-Content $log "listening on 224.0.1.55:2989"
  $deadline = (Get-Date).AddSeconds(12); $n = 0
  while ((Get-Date) -lt $deadline) {
    try {
      $ep = New-Object Net.IPEndPoint([Net.IPAddress]::Any, 0)
      $data = $udp.Receive([ref]$ep)
      $n++
      $hex = ($data | ForEach-Object { $_.ToString('x2') }) -join ''
      Add-Content $log ("REQ #$n from $($ep.Address):$($ep.Port) len=$($data.Length) hex=$hex")
      $reply = New-Object Net.Sockets.UdpClient   # echo the bytes back (unicast)
      [void]$reply.Send($data, $data.Length, $ep)
      $reply.Close()
    } catch { if ($_.Exception.GetType().Name -ne 'SocketException') { Add-Content $log ("ERR " + $_.Exception.Message) } }
  }
  Add-Content $log "done, received=$n"
  $udp.Close()
} catch { Add-Content $log ("FATAL " + $_.Exception.Message) }
