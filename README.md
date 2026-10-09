<p align="center">
<img width="1086" src="res/bahamut_banner.png" alt="BahamutXIV banner">
</p>

<h1 align="center">Bahamut Launcher</h1>

<p align="center">
Launcher for the BahamutXIV Final Fantasy XIV 1.23b emulation server.
</p>

<p align="center">
<a href="LICENSE.md"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
<a href="https://github.com/BahamutXIV/bahamut-launcher/actions/workflows/ci.yml"><img src="https://github.com/BahamutXIV/bahamut-launcher/actions/workflows/ci.yml/badge.svg" alt="Checks"></a>
<a href="https://discord.gg/PxK5RJYQjm"><img src="https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&amp;logoColor=white" alt="Join Discord"></a>
</p>

## Install

[Download the latest release](https://github.com/BahamutXIV/bahamut-launcher/releases),
then follow [getting started](docs/getting-started.md) to install the launcher
and game.

## System requirements

- **Windows x86_64:** the launcher installs missing WebView2 and x86 Visual
  C++ runtimes when needed.
- **Linux x86_64:** SteamOS and Steam Deck use the [Flatpak package](docs/flatpak.md),
  the `linux-flatpak.zip` download on the releases page.
  The Linux archive requires glibc 2.35 or newer, WebKitGTK 4.1, and GTK 3. Run the
  extracted tar.gz in place or install it with an application menu entry.
  Wine downloads on first game launch.
- **macOS, Apple Silicon or Intel:** a universal app with Wine downloaded on
  first game launch. Apple Silicon also needs Rosetta 2 for Wine.

macOS game launch and client extensions remain unverified against a live client.
See [getting started](docs/getting-started.md#platform-requirements)
for the full requirements and limitations.

## Documentation

- [Getting started](docs/getting-started.md): install and launch
- [Configuration](docs/configuration.md): settings, files, and backups
- [Troubleshooting](docs/troubleshooting.md): diagnose startup, install, and launch problems
- [Documentation index](docs/README.md): player guides and developer reference

## Acknowledgement

- SeventhUmbral

## License

<a href="LICENSE.md"><img src="https://upload.wikimedia.org/wikipedia/commons/f/f8/License_icon-mit-88x31-2.svg" width="88" height="31" alt="MIT License"></a>

This unofficial project is not affiliated with or endorsed by Square Enix.
