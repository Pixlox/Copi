!macro NSIS_HOOK_POSTINSTALL
  ; Add firewall rules for Copi LAN Sync
  DetailPrint "Configuring Windows Firewall rules for Copi..."

  ; TCP port 51827 - Copi LAN Sync TCP
  ExecWait 'powershell -NoProfile -ExecutionPolicy Bypass -Command "New-NetFirewallRule -DisplayName ''Copi LAN Sync TCP'' -Direction Inbound -Protocol TCP -LocalPort 51827 -Action Allow -Profile Private,Public -ErrorAction SilentlyContinue"' $0

  ; UDP port 5353 - Copi mDNS UDP
  ExecWait 'powershell -NoProfile -ExecutionPolicy Bypass -Command "New-NetFirewallRule -DisplayName ''Copi mDNS UDP'' -Direction Inbound -Protocol UDP -LocalPort 5353 -Action Allow -Profile Private,Public -ErrorAction SilentlyContinue"' $0

  DetailPrint "Firewall rules configured successfully."
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove firewall rules on uninstall
  DetailPrint "Removing Copi firewall rules..."

  ExecWait 'powershell -NoProfile -ExecutionPolicy Bypass -Command "Remove-NetFirewallRule -DisplayName ''Copi LAN Sync TCP'' -ErrorAction SilentlyContinue"' $0
  ExecWait 'powershell -NoProfile -ExecutionPolicy Bypass -Command "Remove-NetFirewallRule -DisplayName ''Copi mDNS UDP'' -ErrorAction SilentlyContinue"' $0

  DetailPrint "Firewall rules removed."
!macroend
