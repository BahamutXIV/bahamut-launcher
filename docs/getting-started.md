# Getting started

## First launch

1. Download the archive for your system from the
   [releases page](https://github.com/BahamutXIV/bahamut-launcher/releases).
   On Windows, extract the entire archive into a writable folder. On Linux,
   follow [Linux first launch](#linux-first-launch). On macOS, unzip the
   archive (Safari may do this automatically) and optionally move
   `Bahamut Launcher.app` to Applications.
2. Start `bahamut-launcher.exe` on Windows, or `bahamut-launcher` on Linux. On
   macOS, open `Bahamut Launcher.app`; the first launch may ask for access to
   the Documents folder, where the retail config file and the download cache
   folder live, and, when a server profile points at a machine on the local
   network, for local network access.
3. Choose Install to download the game into a new or empty folder. The default
   is `C:\Games\FINAL FANTASY XIV` on Windows or `~/Games/FINAL FANTASY XIV` on
   Linux and macOS. Use PATH to choose a different location before installing.
   Keep the game in its own directory, separate from the launcher.
4. To use an existing, fully updated 1.23b client, select its folder under
   Settings > Misc > Install Location. For an older client, choose Install on
   Home and use a different empty folder.
5. Follow progress on Home. The launcher checks available disk space before
   installation; macOS purgeable storage does not count as free space. When
   installation is ready, choose a server profile, sign in, and start the game.

The launcher archive does not contain the game files. A fresh installation
needs an Internet connection. If installation fails, use the
[troubleshooting guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#install-failures).

## Linux first launch

The Linux archive extracts to a single `bahamut-launcher/` folder. Extract it
into a new or empty directory, never into an older launcher tree:

```bash
tar -xzf bahamut-launcher-vX.Y.Z-linux-x86_64.tar.gz
cd bahamut-launcher
./install-dependencies.sh --check
```

`./install-dependencies.sh --check` reports missing desktop libraries and a
glibc that is too old. It also reports which Wine the launcher uses, the
engine's display, font, and audio libraries, and whether a Vulkan loader and
driver are present.
`./install-dependencies.sh --install` prints the package command for the
detected distribution, asks for confirmation, and runs it with `sudo` or
`doas`. Both commands are optional.

Then run the launcher in place or install it:

- `./bahamut-launcher` runs the launcher from the extracted folder.
- `./install.sh` installs for the current user under `~/.local`: the launcher
  files in `~/.local/lib/bahamut-launcher`, the `bahamut-launcher` command in
  `~/.local/bin`, and an application menu entry with its icons. The menu entry
  works even when `~/.local/bin` is not on `PATH`.
- `sudo ./install.sh` installs the same files under `/usr/local` for every
  user. `sudo make install` in the extracted folder, or
  `sudo make -C <extracted folder> install` from elsewhere, does the same.
  `./install.sh --help` lists the prefix and staging options. `install.sh`
  also reads `PREFIX` and `DESTDIR` from the environment; the payload
  directory is set only with `--pkgdir`.

Run the installed copy of `install.sh` with `--uninstall`, as the same user,
to remove an installation, for example
`~/.local/lib/bahamut-launcher/install.sh --uninstall`.

Whether it runs in place or installed, the launcher keeps its configuration,
logs, and other writable state in `~/.bahamut-launcher`. Replacing,
reinstalling, or uninstalling the launcher leaves that folder in place. See
[Portable archives](#portable-archives).

To update a copy that runs in place, delete or rename the old
`bahamut-launcher/` folder before extracting the new archive, so files that a
newer release drops do not stay behind; the state in `~/.bahamut-launcher` is
not affected. To update an installed copy, run `./install.sh` from the new
archive's folder. A launcher tree that keeps its state beside the executable
needs the steps in
[Moving a beside-the-launcher tree](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#moving-a-beside-the-launcher-tree).

### Gentoo and Hyprland

On Gentoo, install the runtime libraries:

```bash
sudo emerge --ask --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3
```

To use your own Wine instead of the launcher's, set `BAHAMUT_WINE`; it needs
Wine 7 or newer with 32-bit support. Then run `./install.sh` from the
extracted folder. The
[Gentoo package template](https://github.com/BahamutXIV/bahamut-launcher/blob/main/packaging/gentoo/README.md)
installs the same archive through a local overlay instead.

After installation, Bahamut Launcher appears in application launchers that
read desktop entries, such as fuzzel, wofi, and rofi in `drun` mode. To float
the launcher window in Hyprland 0.53 or newer, add this rule to
`hyprland.conf`:

```ini
windowrule = match:class ^([Bb]ahamut-launcher)$, float on
```

Hyprland 0.52 and older use the older rule syntax:

```ini
windowrulev2 = float, class:^([Bb]ahamut-launcher)$
```

On native Wayland the window class is `bahamut-launcher`. Under XWayland,
when GTK is built without Wayland support or `GDK_BACKEND=x11` is set, the
X11 class is `Bahamut-launcher`. The pattern matches both.

## Platform requirements

| System | Requirements |
|---|---|
| Windows x86_64 | WebView2 Runtime and x86 Microsoft Visual C++ Runtime. The launcher installs missing runtimes when needed. |
| Linux x86_64 | WebKitGTK 4.1, GTK 3, and glibc 2.35 or newer. The first game launch downloads a Wine engine; see [Linux Wine engine](configuration.md#linux-wine-engine). The tar.gz archive does not bundle system libraries. |
| macOS, Apple Silicon or Intel | Universal app (`Bahamut Launcher.app`), distributed as a zip. Managed Sikarugir Wine downloads on first game launch. Apple Silicon needs Rosetta 2 for the Wine engine. |

Linux and macOS game launch and client extensions remain unverified against a
live client. See
[platform support](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/extensions.md#platform-support)
for limitations. The Linux archive targets glibc-based distributions; musl
distributions are not supported.

On Windows, missing prerequisites require an Internet connection and may show
an elevation prompt. The WebView2 bootstrapper downloads its runtime from
Microsoft. Restart Windows manually if an installer requests it.

Keep all launcher files together, including `bahamut-loader.exe`,
`bahamut.dll`, and `plugins/`: the extracted folder on Windows, and the
extracted folder or the installed copy on Linux. Opening
`Bahamut Launcher.app` on macOS keeps its files together automatically. On
macOS, use `f1` through `f9` for Screenshot's `hotkey` in
`~/.bahamut-launcher/config/plugins/screenshot/settings.ini` when the
keyboard has no Print Screen key. Screenshot capture under Wine remains
unverified.

## Portable archives

The Windows archive is portable: its settings, logs, and other writable state
stay beside the launcher, so the whole folder can move. The Linux package and
the macOS app keep their writable state in `~/.bahamut-launcher` instead:
configuration, logs, backups, screenshots, scripts, custom addons, custom DAT
packages, and the managed Wine prefix. The macOS app also keeps its managed
Wine engine there, and WebKit keeps its WebView storage under
`~/Library/WebKit`. Linux keeps its Wine engine and a DXVK cache under
`runtime/` in the same folder. The `BAHAMUT_LAUNCHER_HOME` environment
variable, when it holds an absolute path, selects a different folder on Linux
and macOS. The Linux package
marks its folder with a `.bahamut-launcher-package` file. Removing it from a
folder you extracted yourself keeps state beside the launcher instead. Keep
it in a copy that `install.sh` installed, because `install.sh` replaces or
deletes that whole directory; see
[Linux package layout](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md#linux-package-layout).
See
[Configuration](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md)
for exact paths, game options, extensions, and backups.

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
