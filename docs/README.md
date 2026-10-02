# Bahamut Launcher documentation

[Back to the project README](../README.md)

Use these pages to install, configure, build, troubleshoot, and release Bahamut
Launcher. The launcher consumes Bahamut services and does not define server
behavior.

## Guides

- [Getting started](getting-started.md) - platform requirements, archive use,
  Linux install, first launch, and client requirements.
- [Flatpak](flatpak.md) - SteamOS and Steam Deck installation, game
  directories, package permissions, and reproducible builds.
- [Troubleshooting](troubleshooting.md) - build, logs, Linux startup,
  configuration, install, authentication, and recovery diagnosis.
- [Configuration](configuration.md) - portable files, the macOS app and Linux
  package layouts, game settings, backups, extensions, scripts, and DAT
  overlays.
- [DAT overlays](dat-overlays.md) - package layout, selection, and a complete
  replacement example.

## Technical docs

These pages define the requests, files, launch arguments, and release metadata
used by the launcher and its boundaries. Players can usually skip them.

- [Game content delivery](content-delivery.md) - verified downloads,
  staged installation, recovery, and publisher inputs.
- [Signed release metadata](release-metadata.md) - offline signing,
  release ordering, managed inventories, and trust limits.
- [Authentication](auth.md) - account requests, responses, errors,
  and session behavior.
- [Handshake](handshake.md) - launch arguments, session tokens,
  PE patches, and platform launch behavior.
- [Extensions](extensions.md) - Lua addons, native plugins, package
  layout, host APIs, and platform support.

## Development and release

- [Development](development.md) - prerequisites, workspace checks, browser
  checks, native tests, Linux archive packaging, and platform limits.
- [Release process](releasing.md) - merge-to-main version bump and tag
  automation, archive contents, and platform limits.
- [Win32 client module](../client/README.md) - MSVC and llvm-mingw build and
  test commands and their coverage limits.
- [Gentoo package template](../packaging/gentoo/README.md) - local overlay
  ebuild for the Linux release archive.
- [Linux archive README](../packaging/linux/README.md) - the `README.md` that
  ships inside the Linux release archive.

## Public documentation policy

- [AI-assisted contributions](ai_agents/README.md) - ownership and tracked
  documentation policy.
- [Comments and prose](ai_agents/comments-and-prose.md) - source comment and
  public prose rules.
- [Evidence and claims](ai_agents/evidence-and-claims.md) - evidence classes,
  citations, and compatibility wording.
