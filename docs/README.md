# Bahamut Launcher documentation

[Back to the project README](../README.md)

## Play the game

Start with [Getting started](getting-started.md). It covers system
requirements, installation, and first launch.

- [Flatpak](flatpak.md): SteamOS and Steam Deck installation, game directories, package permissions, and builds
- [Configuration](configuration.md): change settings, find your files, and back up or restore data
- [Troubleshooting](troubleshooting.md): fix startup, installation, login, and launch problems
- [Extensions](extensions.md): use addons and plugins, and check platform support
- [DAT overlays](dat-overlays.md): select replacement DAT packages or make your own

## Build and package the launcher

These guides are for developers, package maintainers, and testers.

- [Development](development.md): prerequisites, builds, checks, and tests
- [Win32 client module](../client/README.md): MSVC and llvm-mingw builds and native tests
- [Release process](releasing.md): versioning, tags, release contents, and publishing
- [Linux package README](../packaging/linux/README.md): instructions shipped with the Linux tar.gz
- [Gentoo package template](../packaging/gentoo/README.md): install the Linux release through a local overlay
- [Flatpak build](flatpak.md#build): produce a bundle and inspect its source identity

## Integrate with the launcher

These references describe the launcher's contracts with services, client
modules, and published content. The launcher consumes Bahamut services; these
pages do not define server behavior.

- [Authentication](auth.md): account requests, responses, errors, and sessions
- [Handshake](handshake.md): launch arguments, session tokens, client patches, and platform launch paths
- [Game content delivery](content-delivery.md): download verification, staged installation, recovery, and publisher inputs
- [Signed release metadata](release-metadata.md): signing, release ordering, managed files, and trust limits
- [Extension author reference](extensions.md): package formats, Lua APIs, and native plugins

## Contribution guidance

- [AI-assisted contributions](ai_agents/README.md)
- [Comments and prose](ai_agents/comments-and-prose.md)
- [Evidence and claims](ai_agents/evidence-and-claims.md)
