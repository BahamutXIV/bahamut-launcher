# Bahamut Launcher Flatpak for SteamOS and Linux

Use this Flatpak release to install, repair, and start the FINAL FANTASY XIV
1.23b client for the BahamutXIV server on SteamOS, Steam Deck, and other Linux
systems with Flatpak.

The download contains only the launcher. Choose Install on the Home tab to
download the game.

## Requirements

- Flatpak, which SteamOS normally provides.
- The GNOME 50 runtime, `org.gnome.Platform//50`, from Flathub. The bundle
  names Flathub as its runtime source, so the install command below offers to
  add the Flathub remote and downloads the runtime when it is missing.
- An Internet connection for the runtime, the game, and the Wine engine.

You do not need to install Wine. The launcher downloads its own Wine engine
on the first Play: upstream Wine 11.18 built in WoW64 mode by
Kron4ek/Wine-Builds, a 99,305,644 byte download. See
[Linux Wine engine](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md#linux-wine-engine).

## Install

Unzip the release download, check the bundle, and install it for the current
user. Replace `vX.Y.Z` with the version in the file names:

```bash
unzip bahamut-launcher-vX.Y.Z-linux-flatpak.zip
cd bahamut-launcher-flatpak
sha256sum --check bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak.sha256
flatpak install --user ./bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak
```

Accept the prompts to add Flathub and install the runtime if asked. Then
start the launcher from the application menu entry named Bahamut Launcher or
from a terminal:

```bash
flatpak run io.github.BahamutXIV.Launcher.Tester
```

`flatpak run io.github.BahamutXIV.Launcher.Tester --version` prints the
release version of the installed launcher.

## First launch

1. Start Bahamut Launcher from the application menu or with `flatpak run`.
2. On Home, choose Install and select a new or empty game folder. Read
   [Game directory](#game-directory) before choosing a folder outside the
   default.
3. Choose a server profile and sign in.
4. Choose Play.

The first Play takes longer because it downloads the Wine engine and sets up
its prefix.

## Game directory

The default game destination is
`~/.var/app/io.github.BahamutXIV.Launcher.Tester/data/launcher/game`, inside
the package's private data folder. Install on Home uses it. Use Path on Home
to choose another folder.

The package can write anywhere under `~/Games`. Flatpak creates `~/Games` when
it is absent. Installation needs the parent of the game folder as well,
because it stages the download in a sibling directory named
`<folder>.bahamut-stage` and then publishes it as the game folder.

To install under `~/Games`, for example in `~/Games/Bahamut`:

1. Create an empty `Bahamut` folder inside `~/Games` with the desktop file
   manager.
2. Use Path on Home to select `~/Games/Bahamut`.
3. Confirm that the displayed destination is the plain absolute path,
   `/home/<user>/Games/Bahamut`, before pressing Install. A path shown as
   `/run/user/<uid>/doc/...` or `/run/flatpak/doc/...` is a document-portal
   export and does not give the launcher the access it needs. Pick the folder
   again under `~/Games` if you see one.
4. Complete installation, choose a server profile, sign in, and choose Play.

For an SD card or an external drive, grant the mounted parent folder with a
Flatpak override while the launcher is closed, then reopen it and select the
folder again. SteamOS usually mounts SD cards under `/run/media/`. Check the
actual mount point in Dolphin or with `findmnt` and use that path:

```bash
flatpak override --user --filesystem=<mount point> io.github.BahamutXIV.Launcher.Tester
```

Restarting the launcher keeps the selected game folder.

## Steam Deck

Install and set up the launcher in Desktop Mode. To start it from Game Mode,
add the Bahamut Launcher entry to Steam with Steam's own Add to Steam flow,
for example from the entry's right-click menu in the application launcher.
It then appears in the Steam library like any other non-Steam application.
Adding it to Steam and the controller layout it gets are Steam features
outside the launcher.

## Update and uninstall

To update, unzip the newer release and run the same install command with its
bundle:

```bash
flatpak install --user ./bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak
```

The package keeps the same application ID and branch, so the newer bundle
replaces the installed one in place. Launcher settings, logs, the Wine
prefix, and the game folder stay where they are.

To uninstall the launcher and keep its data:

```bash
flatpak uninstall --user io.github.BahamutXIV.Launcher.Tester
```

Add `--delete-data` to remove
`~/.var/app/io.github.BahamutXIV.Launcher.Tester` as well. That folder holds
the configuration, logs, Wine prefix, Wine engine, and the default game
folder. A game installed under `~/Games` is never removed by Flatpak. Delete
it yourself when you no longer want it.

## Where files live

Launcher state lives in
`~/.var/app/io.github.BahamutXIV.Launcher.Tester/data/launcher`:

| Path | Content |
|---|---|
| `config/` | Configuration files. |
| `logs/launcher/bahamut-launcher.log` | Launcher log. |
| `logs/wine.log` | Wine output. |
| `prefix/` | Managed Wine prefix. |
| `runtime/` | Wine engine and DXVK cache. |
| `game/` | Default game folder. |

The package declares these permissions:

| Permission | Purpose |
|---|---|
| `--share=network` | Launcher login, game, Wine, and DXVK downloads. |
| `--share=ipc` | Desktop toolkit and WebKit IPC. |
| `--socket=fallback-x11`, `--socket=wayland` | Display. |
| `--socket=pulseaudio` | Game audio. |
| `--device=dri` | Graphics device access. |
| `--allow=multiarch` | 32-bit game and loader execution under the WoW64 Wine engine. |
| `--filesystem=~/Games:create` | Game folders and their staging directories under `~/Games`. |

Host filesystem access is limited to `~/Games` and its subdirectories unless
you add an override. To check the installed permissions and your overrides:

```bash
flatpak info --show-permissions io.github.BahamutXIV.Launcher.Tester
flatpak override --user --show io.github.BahamutXIV.Launcher.Tester
```

## Troubleshooting

- A window that opens but stays blank: start the launcher with the WebKitGTK
  override.

  ```bash
  flatpak run --env=WEBKIT_DISABLE_DMABUF_RENDERER=1 io.github.BahamutXIV.Launcher.Tester
  ```

- Install refuses the chosen folder: confirm the displayed path is the plain
  absolute path under `~/Games`, or grant the mount point as described in
  [Game directory](#game-directory).
- Anything else: read the
  [online troubleshooting guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md),
  ask in the [Bahamut Discord](https://discord.gg/PxK5RJYQjm), or report a
  reproducible launcher bug in the
  [issue tracker](https://github.com/BahamutXIV/bahamut-launcher/issues).
  Include the launcher version, the SteamOS or distribution version, the
  steps to reproduce the problem, and a reviewed, redacted copy of the
  launcher log. Do not post passwords or session tokens.

More documentation:

- [Flatpak guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/flatpak.md)
- [Getting started](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/getting-started.md)
- [Configuration](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md)

## What is in this folder

| Path | Purpose |
|---|---|
| `README.md` | This file. |
| `bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak` | The launcher bundle that `flatpak install` reads. |
| `bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak.sha256` | SHA-256 checksum of the bundle for `sha256sum --check`. |
| `bahamut-launcher-vX.Y.Z-linux-flatpak.flatpak.identity.json` | Build identity: source commit, launcher version and, for tag builds, the release tag, toolchain inputs, runtime and SDK commits, and the bundle digest. |

## License

The launcher is released under the MIT license. The installed package keeps
`LICENSE.md` and third-party notices under `licenses/` in
`/app/lib/bahamut-launcher`, readable with
`flatpak run --command=cat io.github.BahamutXIV.Launcher.Tester /app/lib/bahamut-launcher/LICENSE.md`.
