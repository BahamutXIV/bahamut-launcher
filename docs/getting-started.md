# Getting started

## First launch

1. Download an archive from the
   [GitHub releases page](https://github.com/BahamutXIV/bahamut-launcher/releases)
   and extract it completely into a writable folder.
2. Start `bahamut-launcher.exe` on Windows. On Linux or macOS, run
   `./bahamut-launcher` in the extracted folder.
3. Choose Install for a new or empty folder. To use an existing client, choose
   its folder under Settings > Misc > Install Location. The launcher checks
   immediately free disk space before installation and reports failures in the
   Home install strip. macOS purgeable storage does not count. A resolved
   install contains `ffxivboot.exe`. A ready install also contains
   `ffxivgame.exe` and a `game.ver` for the target 1.23b
   build.
4. The shipped manifest contains one complete final 1.23b archive. It does not
   offer incremental remote retail patch objects. For an older client, choose
   Install Fresh on Home and select another empty folder. The configured host
   must still provide the pinned archive. See the [game content delivery
   reference](content-delivery.md) for identities and verification.
5. A fresh install defaults to `C:\Games\FINAL FANTASY XIV` on Windows and
   `~/Games/FINAL FANTASY XIV` on macOS and Linux. Use PATH to change it before
   selecting Install. When the install strip reports Ready, select or edit a
   server profile and sign in.
6. Start the game.

## Platform requirements

| Platform | Archive and runtime | Client module |
|---|---|---|
| Windows x86_64 | ZIP, with WebView2 Runtime | Included and loaded at launch |
| Linux x86_64 | tar.gz, system Wine, and the required Tauri libraries | Included and loaded under Wine at launch. Unverified against a live client. |
| macOS x86_64 | tar.gz, with managed Sikarugir Wine on first game launch | Included and loaded under Wine at launch. Unverified against a live client. |

Linux archives do not bundle system libraries and are not universal
distributions. macOS archives target Intel x86_64 and have no app bundle. Keep
`bahamut-loader.exe` and `bahamut.dll` beside the launcher on both platforms or
the game starts without extensions. On macOS, set the Screenshot `hotkey` in
`config/plugins/screenshot/settings.ini` to `f1` through `f9` because Apple
keyboards have no Print Screen key. See
[extension platform support](extensions.md#platform-support) for the remaining
limits. Building an archive does not establish game launch
compatibility on that platform.

On Windows, the launcher checks WebView2 before opening. If it is missing, it
runs the pinned Microsoft Evergreen bootstrapper, which downloads the runtime
from Microsoft. Before starting the 32-bit loader, it also checks for the x86
Microsoft Visual C++ Runtime and runs the pinned redistributable when needed.
Both installers need an Internet connection and may show a Windows elevation
prompt. Restart Windows manually if an installer requests it.

The launcher does not include retail client files. Keep the client in its own
directory. [Configuration](configuration.md) covers portable settings, game
options, extensions, and backups. [Game content delivery](content-delivery.md)
covers installation and resumable downloads. A build without a pinned base
package can use an existing client, but changing the download host cannot add
missing content identities.

## Portable archives

Each release archive includes the launcher README, the
[MIT license](https://github.com/BahamutXIV/bahamut-launcher/blob/main/LICENSE.md),
and notices for bundled libraries and fonts. Archives also carry the packaged
x86 loader, runtime, Screenshot and DiscordRPC plugins, repository addons, and
the official DAT overlay. Windows archives add the launcher update helper.

Run the executable from the extracted directory. Portable settings remain
beside it. Managed Wine data on Linux and macOS uses the platform user data
directory. See [Release process](releasing.md) for archive names and checksum
sidecars.

## Support

Start with the [troubleshooting guide](troubleshooting.md).
Include the launcher version, platform, install state, and a redacted support
log when reporting a problem in the
[issue tracker](https://github.com/BahamutXIV/bahamut-launcher/issues).
