<!-- Keep asset names in sync with client.yml. __TAG__ is replaced at release time. -->

## Arcagrad Client
The client app connects to an Arcagrad server. If you do not have a server yet, follow the [server installation guide](https://github.com/KalininG/arcagrad#readme) first.

### Downloads

| Platform | Download | Installation |
|---|---|---|
| **Windows** | [Windows installer](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-Windows-Setup.exe) | Open the downloaded `.exe`. |
| **macOS** — Apple silicon | [macOS disk image](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-macOS-AppleSilicon.dmg) | Open the `.dmg` and drag Arcagrad into Applications. |
| **Linux** — any distribution | [AppImage](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-Linux-x86_64.AppImage) | Mark it as executable, then open it. |
| **Ubuntu, Debian, Mint, Pop!_OS** | [Debian package](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-Linux-amd64.deb) | Open the `.deb` with your package installer. |
| **Fedora, RHEL, openSUSE** | [RPM package](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-Linux-x86_64.rpm) | Open the `.rpm` with your package installer. |
| **Android** | [APK](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-Android.apk) | Allow "install unknown apps" for your browser/files app, then open the `.apk`. |
| **iOS** — sideload | [IPA](https://github.com/KalininG/arcagrad/releases/download/__TAG__/Arcagrad-iOS.ipa) | The `.ipa` is unsigned; install it with [AltStore](https://altstore.io) or [Sideloadly](https://sideloadly.io), which re-sign it with your own Apple ID. |

The macOS build currently supports Apple silicon Macs only.

### First launch

These preview builds are not code-signed, so macOS or Windows may show a warning.

- **macOS:** If macOS reports that the app is damaged, open Terminal and run:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Arcagrad.app
  ```

- **Windows:** If Windows protects your PC, select **More info**, then **Run anyway**.

Report problems in [GitHub Issues](https://github.com/KalininG/arcagrad/issues).
