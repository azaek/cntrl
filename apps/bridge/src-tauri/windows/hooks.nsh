; NSIS Installer Hooks for Cntrl Bridge

; Hook: Runs after installation completes
!macro NSIS_HOOK_POSTINSTALL
    ; Write default config.json to user's AppData
    SetShellVarContext current
    CreateDirectory "$APPDATA\com.azaek.cntrl-bridge"

    ; Only create config if it doesn't exist (preserve existing config)
    IfFileExists "$APPDATA\com.azaek.cntrl-bridge\config.json" skipConfig

    ; Write JSON config file with default values
    FileOpen $0 "$APPDATA\com.azaek.cntrl-bridge\config.json" w
    FileWrite $0 "{$\r$\n"
    FileWrite $0 '  "server": {$\r$\n'
    FileWrite $0 '    "port": 9990,$\r$\n'
    FileWrite $0 '    "host": "0.0.0.0"$\r$\n'
    FileWrite $0 "  },$\r$\n"
    FileWrite $0 '  "display": {$\r$\n'
    FileWrite $0 '    "hostname": ""$\r$\n'
    FileWrite $0 "  },$\r$\n"
    FileWrite $0 '  "features": {$\r$\n'
    FileWrite $0 '    "enable_shutdown": false,$\r$\n'
    FileWrite $0 '    "enable_restart": false,$\r$\n'
    FileWrite $0 '    "enable_hibernate": true,$\r$\n'
    FileWrite $0 '    "enable_sleep": true,$\r$\n'
    FileWrite $0 '    "enable_system": true,$\r$\n'
    FileWrite $0 '    "enable_usage": true,$\r$\n'
    FileWrite $0 '    "enable_stats": false,$\r$\n'
    FileWrite $0 '    "enable_media": true,$\r$\n'
    FileWrite $0 '    "enable_processes": true,$\r$\n'
    FileWrite $0 '    "enable_stream": true,$\r$\n'
    FileWrite $0 '    "enable_autostart": true$\r$\n'
    FileWrite $0 "  },$\r$\n"
    FileWrite $0 '  "stats": {$\r$\n'
    FileWrite $0 '    "gpu_enabled": true,$\r$\n'
    FileWrite $0 '    "disk_cache_seconds": 30,$\r$\n'
    FileWrite $0 '    "stream_interval_seconds": 2$\r$\n'
    FileWrite $0 "  },$\r$\n"
    FileWrite $0 '  "auth": {$\r$\n'
    FileWrite $0 '    "enabled": false,$\r$\n'
    FileWrite $0 '    "api_key": null,$\r$\n'
    FileWrite $0 '    "allowed_ips": []$\r$\n'
    FileWrite $0 "  }$\r$\n"
    FileWrite $0 "}$\r$\n"
    FileClose $0

    skipConfig:
    SetShellVarContext all

    ; Add firewall rule
    DetailPrint "Adding Firewall Rule for Cntrl Bridge..."
    nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Cntrl Bridge" dir=in action=allow program="$INSTDIR\Cntrl Bridge.exe" enable=yes'
!macroend

; Hook: Runs before uninstallation
!macro NSIS_HOOK_PREUNINSTALL
    ; Remove firewall rule
    DetailPrint "Removing Firewall Rule..."
    nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Cntrl Bridge"'

    ; Note: We preserve the config file in AppData so user settings are kept for reinstall
!macroend
