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

Download the latest version from the [releases page](https://github.com/BahamutXIV/bahamut-launcher/releases),
then follow [Getting started](docs/getting-started.md) for platform requirements,
installation, and first launch steps.

## System requirements

- Windows x86_64. The launcher installs missing WebView2 and x86 Visual C++
  runtimes when needed.
- Linux x86_64. SteamOS and Steam Deck use the [Flatpak package](docs/flatpak.md).
  The Linux archive requires glibc 2.35 or newer, WebKitGTK 4.1, and GTK 3. The
  launcher downloads its own Wine on the first game launch. The archive runs
  in place or installs with an application menu entry.
- macOS on Apple Silicon or Intel as a universal app, with managed Wine
  downloaded on first game launch. Apple Silicon needs Rosetta 2 for the
  Wine engine.

See [Getting started](docs/getting-started.md) for complete system requirements.

## Bug reports

Start with [Troubleshooting](docs/troubleshooting.md). For further assistance,
[join the Bahamut Discord](https://discord.gg/PxK5RJYQjm). Report reproducible
launcher bugs and focused feature requests in the [issue tracker](https://github.com/BahamutXIV/bahamut-launcher/issues).

## Documentation

Use the [documentation index](docs/README.md) for setup, configuration,
extension packages, and troubleshooting.

## Acknowledgement

- SeventhUmbral

## License

<a href="LICENSE.md"><img src="https://upload.wikimedia.org/wikipedia/commons/f/f8/License_icon-mit-88x31-2.svg" width="88" height="31" alt="MIT License"></a>
