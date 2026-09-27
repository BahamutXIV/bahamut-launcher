# Getting started

## First launch

1. Download the archive for your system from the
   [releases page](https://github.com/BahamutXIV/bahamut-launcher/releases).
   Extract the entire archive into a writable folder.
2. Start `bahamut-launcher.exe` on Windows. On Linux or macOS, run
   `./bahamut-launcher` from the extracted folder.
3. Choose Install to download the game into a new or empty folder. The default
   is `C:\Games\FINAL FANTASY XIV` on Windows or `~/Games/FINAL FANTASY XIV` on
   Linux and macOS. Use PATH to choose a different location before installing.
   Keep the game in its own directory, separate from the launcher.
4. To use an existing, fully updated 1.23b client, select its folder under
   Settings > Misc > Install Location. For an older client, choose Install
   Fresh on Home and use a different empty folder.
5. Follow progress on Home. The launcher checks available disk space before
   installation; macOS purgeable storage does not count as free space. When
   installation is ready, choose a server profile, sign in, and start the game.

The launcher archive does not contain the game files. A fresh installation
needs an Internet connection. If installation fails, use the
[troubleshooting guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#install-failures).

## Platform requirements

| System | Requirements |
|---|---|
| Windows x86_64 | WebView2 Runtime and x86 Microsoft Visual C++ Runtime. The launcher installs missing runtimes when needed. |
| Linux x86_64 | System Wine and the required desktop libraries. The tar.gz archive does not bundle system libraries. |
| macOS x86_64 | Intel executable, distributed as a tar.gz archive without an app bundle. Managed Sikarugir Wine downloads on first game launch. |

Linux and macOS game launch and client extensions remain unverified against a
live client. See
[platform support](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/extensions.md#platform-support)
for limitations and the
[Linux dependency list](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/development.md#tauri-and-webview)
for the required desktop libraries. Linux archives are not universal packages
for every distribution.

On Windows, missing prerequisites require an Internet connection and may show
an elevation prompt. The WebView2 bootstrapper downloads its runtime from
Microsoft. Restart Windows manually if an installer requests it.

Keep all extracted launcher files together, including `bahamut-loader.exe`,
`bahamut.dll`, and `plugins/`. On macOS, use `f1` through `f9` for Screenshot's
`hotkey` in `config/plugins/screenshot/settings.ini` when the keyboard has no
Print Screen key. Screenshot capture under Wine remains unverified.

## Portable archives

Portable settings remain beside the launcher. Managed Wine data on Linux and
macOS uses the system's user data directory. See
[Configuration](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md)
for game options, extensions, and backups.

Release archives include `LICENSE.md` and the bundled library and font notices
under `licenses/`. SHA-256 checksum files are supplied beside the archives on
the releases page. Keep these notices with the launcher.

## Support

Start with
[Troubleshooting](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md)
or ask for help in the [Bahamut Discord](https://discord.gg/PxK5RJYQjm).
Report reproducible launcher bugs in the
[issue tracker](https://github.com/BahamutXIV/bahamut-launcher/issues).
Include the launcher version, operating system, steps to reproduce the problem,
and a reviewed, redacted support log. Do not post passwords, session tokens,
or private packet captures.
