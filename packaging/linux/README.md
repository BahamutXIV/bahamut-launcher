# Bahamut Launcher for Linux

Use this Linux x86_64 release to install, repair, and start the FINAL FANTASY XIV
1.23b client for the BahamutXIV server.

The download contains only the launcher. Choose Install on the Home tab to
download the game.

## Requirements

- An x86_64 machine with a glibc-based distribution and glibc 2.35 or newer.
- WebKitGTK 4.1 and GTK 3.
- `tar` and `xz` to unpack the Wine download.

You do not need to install Wine; the launcher downloads it on the first Play.
A Vulkan driver enables DXVK. Without one, the game uses the slower OpenGL
renderer.

## Quick start

Check dependencies with [install-dependencies.sh](install-dependencies.sh),
then start the launcher from this folder:

```bash
./install-dependencies.sh --check
./bahamut-launcher
```

`./install-dependencies.sh --check` reports missing libraries, an unsupported
glibc version, the Wine the launcher will use, and the availability of a Vulkan
loader and driver.

To install missing dependencies, run `./install-dependencies.sh --install`.
It prints your distribution's package command, asks for confirmation, and runs
it through `sudo` or `doas`. Add `--yes` to skip confirmation. Both the check
and install commands are optional. See the script's usage text for all options
and exit codes.

## Install

Use [install.sh](install.sh) to put the launcher on your `PATH` and in the
application menu. The [Makefile](Makefile) wraps the same script. Choose one:

```bash
./install.sh
sudo ./install.sh
sudo make install
```

- `./install.sh` installs for the current user under `~/.local`.
- `sudo ./install.sh` installs for all users under `/usr/local`.
- `sudo make install` also installs under `/usr/local`. `make` reads `PREFIX`
  and `DESTDIR`; for example, `make install PREFIX="$HOME/.local"`.

Paths are relative to the install prefix:

| Path | Content |
|---|---|
| `lib/bahamut-launcher/` | All files from this folder except the `Makefile`, including `install.sh` and `install-dependencies.sh`. |
| `bin/bahamut-launcher` | A link to the executable in the payload directory. |
| `share/applications/bahamut-launcher.desktop` | The application menu entry. |
| `share/icons/hicolor/<size>/apps/bahamut-launcher.png` | Icons at 48x48, 128x128, and 256x256. |

The menu entry works even if `~/.local/bin` is absent from your `PATH`.

`install.sh` accepts these options:

- `--prefix DIR`: install prefix; defaults to `~/.local`, or `/usr/local` as root.
- `--pkgdir DIR`: payload directory; defaults to `PREFIX/lib/bahamut-launcher`.
- `--destdir DIR`: staging root prepended to every written path.
- `--skip-checks`: skip the dependency check before installation.
- `--force`: replace or remove a payload directory created by `install.sh`,
  even if it no longer matches the install records. This deletes everything
  in that directory.
- `--uninstall`: remove an installation.
- `--help`: show usage.

The script also reads `PREFIX` and `DESTDIR` from the environment. Run
`./install.sh --help` for exact usage.

## Update and uninstall

For a copy run directly from an extracted folder, delete or rename the old
`bahamut-launcher/` folder first. Extract the new tar.gz into a new or empty
directory. Settings remain in `~/.bahamut-launcher`.

For an installed copy, run `./install.sh` from the new release's folder with
the original install options. It replaces the payload directory.

To uninstall, use the same user account that installed it. For `~/.local`:

```bash
~/.local/lib/bahamut-launcher/install.sh --uninstall
```

For `/usr/local`:

```bash
sudo /usr/local/lib/bahamut-launcher/install.sh --uninstall
```

You can also run `sudo make uninstall` from this folder for a `/usr/local`
installation, or `./install.sh --uninstall` for a `~/.local` installation.
If you used another `--prefix`, `--pkgdir`, or `--destdir`, pass the same
options with `--uninstall`.

Uninstall leaves `~/.bahamut-launcher` intact. To remove a copy you run directly
from an extracted folder, delete that folder. Deleting `~/.bahamut-launcher` as
well removes your settings, logs, and Wine prefix.

## First launch

1. Start `bahamut-launcher` from the command line or application menu, or run
   `./bahamut-launcher` from this folder.
2. On Home, choose Install and select a new or empty game folder. The default
   is `~/Games/FINAL FANTASY XIV`. Keep it separate from the launcher folder.
3. Choose a server profile and sign in.
4. Choose Play.

The first Play takes longer because it downloads Wine and sets up its prefix.

## Where files live

Writable state lives in `~/.bahamut-launcher`:

| Path | Content |
|---|---|
| `config/` | Configuration files. |
| `logs/launcher/bahamut-launcher.log` | Launcher log. |
| `logs/wine.log` | Wine output. |
| `prefix/` | Managed Wine prefix. |
| `runtime/` | Wine engine and DXVK cache. |

Set `BAHAMUT_LAUNCHER_HOME` to an absolute path to use another state directory.

The `.bahamut-launcher-package` file beside `bahamut-launcher` selects this
layout. Keep the marker in an installed copy: `install.sh` replaces or deletes
the whole payload directory and refuses to do so without it.

In a folder you extracted yourself, removing the marker makes the launcher store
configuration and logs beside the executable. The Wine prefix, engine, DXVK cache, and
`wine.log` still live in `~/.bahamut-launcher`.

## Using another Wine

Set `BAHAMUT_WINE` to the Wine executable you want to use:

```bash
BAHAMUT_WINE=/opt/wine/bin/wine bahamut-launcher
```

The launcher uses that file as given. It must be Wine 7 or newer with 32-bit
support. When the variable is set, `./install-dependencies.sh --check` checks
that Wine.

## Desktop integration

After installation, Bahamut Launcher appears in application menus and desktop
entry launchers such as fuzzel, wofi, and rofi in `drun` mode. For a Hyprland
rule that floats the window, see the
[online getting-started guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/getting-started.md#gentoo-and-hyprland).

## What is in this folder

| Path | Purpose |
|---|---|
| `bahamut-launcher` | Launcher executable. |
| `install.sh` | Install and uninstall script. |
| `install-dependencies.sh` | Dependency checker and installer. |
| `Makefile` | `make install` and `make uninstall` targets. |
| `.bahamut-launcher-package` | Package marker described above. |
| `bahamut-loader.exe`, `bahamut.dll` | Client loader and module used inside Wine. |
| `plugins/`, `addons/` | Shipped native plugins, addons, and official DAT package. |
| `scripts/` | Seed for the default script file. |
| `share/` | Menu entry and icons installed by `install.sh`. |
| `LICENSE.md`, `licenses/` | Launcher license and third-party notices. |
| `README.md` | This file. |

## Troubleshooting and documentation

- [Getting started](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/getting-started.md)
- [Configuration](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md)
- [Troubleshooting](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md)
- [Gentoo package template](https://github.com/BahamutXIV/bahamut-launcher/blob/main/packaging/gentoo/README.md)

## License

Read `LICENSE.md` for the launcher license and `licenses/` for third-party
notices.
