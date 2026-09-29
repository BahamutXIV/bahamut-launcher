# Bahamut Launcher for Linux

Bahamut Launcher installs, repairs, and starts the FINAL FANTASY XIV 1.23b
client for the BahamutXIV server. This folder is the Linux x86_64 release of
the launcher.

The archive does not contain the game. The launcher downloads the game client
when you choose Install on its Home tab.

## Requirements

- An x86_64 machine running a glibc-based distribution with glibc 2.35 or
  newer.
- WebKitGTK 4.1 and GTK 3.
- `tar` and `xz`, which the launcher uses to unpack its Wine download.

Wine is not required. The launcher downloads its own Wine on the first Play.
A Vulkan driver lets the game use DXVK; without one the game uses the slower
OpenGL renderer.

## Quick start

Check the dependencies with
[install-dependencies.sh](install-dependencies.sh), then run the launcher from
this folder:

```bash
./install-dependencies.sh --check
./bahamut-launcher
```

`./install-dependencies.sh --check` reports missing libraries, a glibc that is
too old, which Wine the launcher will use, and whether a Vulkan loader and
driver are present. `./install-dependencies.sh --install` prints the package
command for your distribution, asks for confirmation, and runs it through
`sudo` or `doas`; add `--yes` to skip the confirmation. Both commands are
optional. The script's usage text lists every option and exit code.

## Install

[install.sh](install.sh) puts the launcher on your `PATH` and in your
application menu. The [Makefile](Makefile) wraps it.

```bash
./install.sh
sudo ./install.sh
sudo make install
```

- `./install.sh` installs for the current user under `~/.local`.
- `sudo ./install.sh` installs for every user under `/usr/local`.
- `sudo make install` installs under `/usr/local` as well. `make` reads
  `PREFIX` and `DESTDIR`, for example `make install PREFIX="$HOME/.local"`.

The install writes these files, relative to the prefix:

| Path | Content |
|---|---|
| `lib/bahamut-launcher/` | The launcher payload: this folder's files except the `Makefile`, including `install.sh` and `install-dependencies.sh`. |
| `bin/bahamut-launcher` | A link to the launcher in the payload directory. |
| `share/applications/bahamut-launcher.desktop` | The application menu entry. |
| `share/icons/hicolor/<size>/apps/bahamut-launcher.png` | The icons, in 48x48, 128x128, and 256x256. |

The menu entry works even when `~/.local/bin` is not on your `PATH`.

`install.sh` takes these options:

- `--prefix DIR` sets the install prefix; the default is `~/.local`, or
  `/usr/local` as root.
- `--pkgdir DIR` sets the payload directory; the default is
  `PREFIX/lib/bahamut-launcher`.
- `--destdir DIR` sets a staging root that is prepended to every written path.
- `--skip-checks` skips the dependency check that runs before an install.
- `--force` replaces or removes a payload directory that `install.sh` created
  even when it no longer matches its install records. Everything in that
  directory is deleted.
- `--uninstall` removes an installation.
- `--help` shows the usage text.

`install.sh` also reads `PREFIX` and `DESTDIR` from the environment. Run
`./install.sh --help` for the exact usage.

## Update and uninstall

To update a copy that runs in place, delete or rename the old
`bahamut-launcher/` folder, then extract the new archive into a new or empty
directory. Your settings stay in `~/.bahamut-launcher`.

To update an installed copy, run `./install.sh` from the new archive's folder
with the same options as the first install. It replaces the payload directory.

To uninstall, run `--uninstall` as the user who installed. For an install
under `~/.local`:

```bash
~/.local/lib/bahamut-launcher/install.sh --uninstall
```

For an install under `/usr/local`:

```bash
sudo /usr/local/lib/bahamut-launcher/install.sh --uninstall
```

From this folder, `sudo make uninstall` removes an install under `/usr/local`,
and `./install.sh --uninstall` removes one under `~/.local`. For any other
`--prefix`, `--pkgdir`, or `--destdir`, pass the same options to
`--uninstall`.

Uninstalling leaves `~/.bahamut-launcher` in place. To remove a copy that runs
in place, delete this folder. Delete `~/.bahamut-launcher` as well to remove
your settings, logs, and Wine prefix.

## First launch

1. Start the launcher with `bahamut-launcher`, from the application menu, or
   with `./bahamut-launcher` in this folder.
2. On Home, choose Install to download the game into a new or empty folder. The
   default is `~/Games/FINAL FANTASY XIV`. Keep the game in its own directory,
   separate from the launcher.
3. Choose a server profile and sign in.
4. Choose Play.

The first Play downloads Wine and sets up the Wine prefix, so it takes longer
than later ones.

## Where files live

The launcher keeps its writable state in `~/.bahamut-launcher`:

| Path | Content |
|---|---|
| `config/` | Configuration files. |
| `logs/launcher/bahamut-launcher.log` | The launcher log. |
| `logs/wine.log` | Wine's own output. |
| `prefix/` | The managed Wine prefix. |
| `runtime/` | The Wine engine and the DXVK cache. |

If `BAHAMUT_LAUNCHER_HOME` holds an absolute path, the launcher uses that
directory instead of `~/.bahamut-launcher`.

The launcher uses this location because the file `.bahamut-launcher-package`
sits beside `bahamut-launcher`. Keep it in an installed copy: `install.sh`
replaces or deletes that whole directory and refuses to when the file is
missing. In a folder you extracted yourself, removing it makes the launcher
keep its configuration and logs beside the executable; the Wine prefix, the
Wine engine, the DXVK cache, and `wine.log` stay in `~/.bahamut-launcher`.

## Using another Wine

`BAHAMUT_WINE` names the Wine executable to use instead of the launcher's own:

```bash
BAHAMUT_WINE=/opt/wine/bin/wine bahamut-launcher
```

The launcher uses that file as given. It must be Wine 7 or newer with 32-bit
support. With `BAHAMUT_WINE` set, `./install-dependencies.sh --check` checks
that Wine.

## Desktop integration

After `install.sh` runs, Bahamut Launcher appears in application menus and in
launchers that read desktop entries, such as fuzzel, wofi, and rofi in `drun`
mode. The Hyprland window rule for floating the launcher window is in the
[online getting-started guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/getting-started.md#gentoo-and-hyprland).

## What is in this folder

| Path | Purpose |
|---|---|
| `bahamut-launcher` | The launcher executable. |
| `install.sh` | Installs or uninstalls the launcher. |
| `install-dependencies.sh` | Checks for, or installs, the libraries the launcher needs. |
| `Makefile` | The `make install` and `make uninstall` targets. |
| `.bahamut-launcher-package` | The package marker described above. |
| `bahamut-loader.exe`, `bahamut.dll` | The client loader and module that run inside Wine. |
| `plugins/`, `addons/` | Shipped native plugins, addons, and the official DAT package. |
| `scripts/` | The seed for the default script file. |
| `share/` | The menu entry and icons that `install.sh` installs. |
| `LICENSE.md`, `licenses/` | The launcher license and third-party notices. |
| `README.md` | This file. |

## Troubleshooting and documentation

- [Getting started](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/getting-started.md)
- [Configuration](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md)
- [Troubleshooting](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md)
- [Gentoo package template](https://github.com/BahamutXIV/bahamut-launcher/blob/main/packaging/gentoo/README.md)

## License

The launcher license is in `LICENSE.md`. Third-party notices are in the
`licenses/` directory.
